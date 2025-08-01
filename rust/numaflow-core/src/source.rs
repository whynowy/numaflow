//! [Source] vertex is responsible for reliable reading data from an unbounded source into Numaflow
//! and also assigning [Watermark].
//!
//! [Source]: https://numaflow.numaproj.io/user-guide/sources/overview/
//! [Watermark]: https://numaflow.numaproj.io/core-concepts/watermarks/

use crate::config::pipeline::VERTEX_TYPE_SOURCE;
use crate::config::{get_vertex_name, is_mono_vertex};
use crate::error::{Error, Result};
use crate::message::ReadAck;
use crate::metrics::{
    PIPELINE_PARTITION_NAME_LABEL, monovertex_metrics, mvtx_forward_metric_labels,
    pipeline_metric_labels, pipeline_metrics,
};
use crate::source::http::CoreHttpSource;
use crate::tracker::TrackerHandle;
use crate::{
    message::{Message, Offset},
    reader::LagReader,
};
use backoff::retry::Retry;
use backoff::strategy::fixed;
use chrono::Utc;
use numaflow_kafka::source::KafkaSource;
use numaflow_nats::jetstream::JetstreamSource;
use numaflow_nats::nats::NatsSource;
use numaflow_pb::clients::source::source_client::SourceClient;
use numaflow_pulsar::source::PulsarSource;
use numaflow_sqs::source::SqsSource;
use std::sync::Arc;
use tokio::sync::OwnedSemaphorePermit;
use tokio::sync::Semaphore;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tonic::transport::Channel;
use tracing::warn;
use tracing::{error, info};

/// [User-Defined Source] extends Numaflow to add custom sources supported outside the builtins.
///
/// [User-Defined Source]: https://numaflow.numaproj.io/user-guide/sources/user-defined-sources/
pub(crate) mod user_defined;

/// [Generator] is a builtin to generate data for load testing and other internal use-cases.
///
/// [Generator]: https://numaflow.numaproj.io/user-guide/sources/generator/
pub(crate) mod generator;

/// [Pulsar] is a builtin to ingest data from a Pulsar topic
///
/// [Pulsar]: https://numaflow.numaproj.io/user-guide/sources/pulsar/
pub(crate) mod pulsar;

pub(crate) mod jetstream;
pub(crate) mod nats;

pub(crate) mod sqs;

pub(crate) mod http;
pub(crate) mod kafka;

use crate::transformer::Transformer;
use crate::watermark::source::SourceWatermarkHandle;

const MAX_ACK_PENDING: usize = 10000;
const ACK_RETRY_INTERVAL: u64 = 100;
const ACK_RETRY_ATTEMPTS: usize = usize::MAX;

/// Set of Read related items that has to be implemented to become a Source.
pub(crate) trait SourceReader {
    #[allow(dead_code)]
    /// Name of the source.
    fn name(&self) -> &'static str;

    async fn read(&mut self) -> Result<Vec<Message>>;

    /// number of partitions processed by this source.
    async fn partitions(&mut self) -> Result<Vec<u16>>;
}

/// Set of Ack related items that has to be implemented to become a Source.
pub(crate) trait SourceAcker {
    /// acknowledge an offset. The implementor might choose to do it in an asynchronous way.
    async fn ack(&mut self, _: Vec<Offset>) -> Result<()>;
}

pub(crate) enum SourceType {
    UserDefinedSource(
        Box<user_defined::UserDefinedSourceRead>,
        Box<user_defined::UserDefinedSourceAck>,
        user_defined::UserDefinedSourceLagReader,
    ),
    Generator(
        generator::GeneratorRead,
        generator::GeneratorAck,
        generator::GeneratorLagReader,
    ),
    Pulsar(PulsarSource),
    Sqs(SqsSource),
    Jetstream(JetstreamSource),
    Kafka(KafkaSource),
    Http(CoreHttpSource),
    Nats(NatsSource),
}

enum ActorMessage {
    #[allow(dead_code)]
    Name {
        respond_to: oneshot::Sender<&'static str>,
    },
    Read {
        respond_to: oneshot::Sender<Result<Vec<Message>>>,
    },
    Ack {
        respond_to: oneshot::Sender<Result<()>>,
        offsets: Vec<Offset>,
    },
    Pending {
        respond_to: oneshot::Sender<Result<Option<usize>>>,
    },
    Partitions {
        respond_to: oneshot::Sender<Result<Vec<u16>>>,
    },
}

struct SourceActor<R, A, L> {
    receiver: mpsc::Receiver<ActorMessage>,
    reader: R,
    acker: A,
    lag_reader: L,
}

impl<R, A, L> SourceActor<R, A, L>
where
    R: SourceReader,
    A: SourceAcker,
    L: LagReader,
{
    fn new(receiver: mpsc::Receiver<ActorMessage>, reader: R, acker: A, lag_reader: L) -> Self {
        Self {
            receiver,
            reader,
            acker,
            lag_reader,
        }
    }

    async fn handle_message(&mut self, msg: ActorMessage) {
        match msg {
            ActorMessage::Name { respond_to } => {
                let name = self.reader.name();
                let _ = respond_to.send(name);
            }
            ActorMessage::Read { respond_to } => {
                let msgs = self.reader.read().await;
                let _ = respond_to.send(msgs);
            }
            ActorMessage::Ack {
                respond_to,
                offsets,
            } => {
                let ack = self.acker.ack(offsets).await;
                let _ = respond_to.send(ack);
            }
            ActorMessage::Pending { respond_to } => {
                let pending = self.lag_reader.pending().await;
                let _ = respond_to.send(pending);
            }
            ActorMessage::Partitions { respond_to } => {
                let partitions = self.reader.partitions().await;
                let _ = respond_to.send(partitions);
            }
        }
    }

    async fn run(mut self) {
        while let Some(msg) = self.receiver.recv().await {
            self.handle_message(msg).await;
        }
    }
}

/// Source is used to read, ack, and get the pending messages count from the source.
/// Source is responsible for invoking the transformer.
///
/// Error handling and shutdown: Source will stop reading messages from the source when the
/// cancellation token is cancelled(any downstream critical error). There can be critical
/// non retryable errors in source as well(udsource crashing etc.), we will drop the downstream
/// tokio stream to signal the shutdown to the downstream components and wait for all the inflight
/// messages to be acked before shutting down the source.
#[derive(Clone)]
pub(crate) struct Source {
    read_batch_size: usize,
    sender: mpsc::Sender<ActorMessage>,
    tracker_handle: TrackerHandle,
    read_ahead: bool,
    /// Transformer handler for transforming messages from Source.
    transformer: Option<Transformer>,
    watermark_handle: Option<SourceWatermarkHandle>,
    health_checker: Option<SourceClient<Channel>>,
}

impl Source {
    /// Create a new StreamingSource. It starts the read and ack actors in the background.
    pub(crate) fn new(
        batch_size: usize,
        src_type: SourceType,
        tracker_handle: TrackerHandle,
        read_ahead: bool,
        transformer: Option<Transformer>,
        watermark_handle: Option<SourceWatermarkHandle>,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(batch_size);
        let mut health_checker = None;
        match src_type {
            SourceType::UserDefinedSource(reader, acker, lag_reader) => {
                health_checker = Some(reader.get_source_client());
                tokio::spawn(async move {
                    let actor = SourceActor::new(receiver, *reader, *acker, lag_reader);
                    actor.run().await;
                });
            }
            SourceType::Generator(reader, acker, lag_reader) => {
                tokio::spawn(async move {
                    let actor = SourceActor::new(receiver, reader, acker, lag_reader);
                    actor.run().await;
                });
            }
            SourceType::Pulsar(pulsar_source) => {
                tokio::spawn(async move {
                    let actor = SourceActor::new(
                        receiver,
                        pulsar_source.clone(),
                        pulsar_source.clone(),
                        pulsar_source,
                    );
                    actor.run().await;
                });
            }
            SourceType::Sqs(sqs_source) => {
                tokio::spawn(async move {
                    let actor = SourceActor::new(
                        receiver,
                        sqs_source.clone(),
                        sqs_source.clone(),
                        sqs_source,
                    );
                    actor.run().await;
                });
            }
            SourceType::Jetstream(jetstream) => {
                tokio::spawn(async move {
                    let actor =
                        SourceActor::new(receiver, jetstream.clone(), jetstream.clone(), jetstream);
                    actor.run().await;
                });
            }
            SourceType::Nats(nats) => {
                tokio::spawn(async move {
                    let actor = SourceActor::new(receiver, nats.clone(), nats.clone(), nats);
                    actor.run().await;
                });
            }
            SourceType::Kafka(kafka) => {
                tokio::spawn(async move {
                    let actor = SourceActor::new(receiver, kafka.clone(), kafka.clone(), kafka);
                    actor.run().await;
                });
            }
            SourceType::Http(http_source) => {
                tokio::spawn(async move {
                    let actor = SourceActor::new(
                        receiver,
                        http_source.clone(),
                        http_source.clone(),
                        http_source,
                    );
                    actor.run().await;
                });
            }
        };

        Self {
            read_batch_size: batch_size,
            sender,
            tracker_handle,
            read_ahead,
            transformer,
            watermark_handle,
            health_checker,
        }
    }

    /// read messages from the source by communicating with the read actor.
    async fn read(source_handle: mpsc::Sender<ActorMessage>) -> Result<Vec<Message>> {
        let (sender, receiver) = oneshot::channel();
        let msg = ActorMessage::Read { respond_to: sender };
        // Ignore send errors. If send fails, so does the recv.await below. There's no reason
        // to check for the same failure twice.
        let _ = source_handle.send(msg).await;
        receiver
            .await
            .map_err(|e| Error::ActorPatternRecv(e.to_string()))?
    }

    /// ack the offsets by communicating with the ack actor.
    async fn ack(source_handle: mpsc::Sender<ActorMessage>, offsets: Vec<Offset>) -> Result<()> {
        let (sender, receiver) = oneshot::channel();
        let msg = ActorMessage::Ack {
            respond_to: sender,
            offsets,
        };
        // Ignore send errors. If send fails, so does the recv.await below. There's no reason
        // to check for the same failure twice.
        let _ = source_handle.send(msg).await;
        receiver
            .await
            .map_err(|e| Error::ActorPatternRecv(e.to_string()))?
    }

    /// get the pending messages count by communicating with the pending actor.
    pub(crate) async fn pending(&self) -> Result<Option<usize>> {
        let (sender, receiver) = oneshot::channel();
        let msg = ActorMessage::Pending { respond_to: sender };
        // Ignore send errors. If send fails, so does the recv.await below. There's no reason
        // to check for the same failure twice.
        let _ = self.sender.send(msg).await;
        receiver
            .await
            .map_err(|e| Error::ActorPatternRecv(e.to_string()))?
    }

    /// get the source partitions from which the source is reading from.
    async fn partitions(source_handle: mpsc::Sender<ActorMessage>) -> Result<Vec<u16>> {
        let (sender, receiver) = oneshot::channel();
        let msg = ActorMessage::Partitions { respond_to: sender };
        // Ignore send errors. If send fails, so does the recv.await below. There's no reason
        // to check for the same failure twice.
        let _ = source_handle.send(msg).await;
        receiver
            .await
            .map_err(|e| Error::ActorPatternRecv(e.to_string()))?
    }

    /// Starts streaming messages from the source. It returns a stream of messages and
    /// a handle to the spawned task.
    pub(crate) fn streaming_read(
        mut self,
        cln_token: CancellationToken,
    ) -> Result<(ReceiverStream<Message>, JoinHandle<Result<()>>)> {
        let (messages_tx, messages_rx) = mpsc::channel(2 * self.read_batch_size);

        let mut pipeline_labels = pipeline_metric_labels(VERTEX_TYPE_SOURCE).clone();
        pipeline_labels.push((
            PIPELINE_PARTITION_NAME_LABEL.to_string(),
            get_vertex_name().to_string(),
        ));

        let mvtx_labels = mvtx_forward_metric_labels();

        info!(?self.read_batch_size, "Started streaming source with batch size");
        let handle = tokio::spawn(async move {
            // this semaphore is used only if read-ahead is disabled. we hold this semaphore to
            // make sure we can read only if the current inflight ones are ack'ed. If read ahead
            // is disabled you can have upto (max_ack_pending / read_batch_size) ack tasks. We
            // divide by read_batch_size because we do batch acking in source.
            let max_ack_tasks = match &self.read_ahead {
                true => MAX_ACK_PENDING / self.read_batch_size,
                false => 1,
            };
            let semaphore = Arc::new(Semaphore::new(max_ack_tasks));
            let mut processed_msgs_count: usize = 0;
            let mut last_logged_at = Instant::now();

            let mut result = Ok(());
            loop {
                if cln_token.is_cancelled() {
                    info!("Cancellation token is cancelled. Stopping the source.");
                    break;
                }

                // Acquire the semaphore permit before reading the next batch to make
                // sure we are not reading ahead and all the inflight messages are acked.
                let _permit = Arc::clone(&semaphore)
                    .acquire_owned()
                    .await
                    .expect("acquiring permit should not fail");

                let read_start_time = Instant::now();
                let messages = match Self::read(self.sender.clone()).await {
                    Ok(messages) => messages,
                    Err(e) => {
                        error!("Error while reading messages: {:?}", e);
                        result = Err(e);
                        break;
                    }
                };

                let msgs_len = messages.len();
                let msgs_bytes = messages.iter().map(|msg| msg.value.len()).sum();
                Self::send_read_metrics(
                    &pipeline_labels,
                    mvtx_labels,
                    read_start_time,
                    msgs_len,
                    msgs_bytes,
                );

                // attempt to publish idle watermark since we are not able to read any message from
                // the source.
                if msgs_len == 0 {
                    if let Some(watermark_handle) = self.watermark_handle.as_mut() {
                        watermark_handle
                            .publish_source_idle_watermark(
                                Self::partitions(self.sender.clone())
                                    .await
                                    .unwrap_or_default(),
                            )
                            .await;
                    }
                }

                let mut offsets = vec![];
                let mut ack_batch = Vec::with_capacity(msgs_len);
                for message in messages.iter() {
                    let (resp_ack_tx, resp_ack_rx) = oneshot::channel();
                    let offset = message.offset.clone();

                    // insert the offset and the ack one shot in the tracker.
                    self.tracker_handle.insert(message, resp_ack_tx).await?;

                    // store the ack one shot in the batch to invoke ack later.
                    ack_batch.push((offset.clone(), resp_ack_rx));
                    offsets.push(offset);
                }

                // start a background task to invoke ack on the source for the offsets that are acked.
                // if read ahead is disabled, acquire the semaphore permit before invoking ack so that
                // we wait for all the inflight messages to be acked before reading the next batch.
                tokio::spawn(Self::invoke_ack(
                    read_start_time,
                    self.sender.clone(),
                    ack_batch,
                    _permit,
                    cln_token.clone(),
                ));

                // transform the batch if the transformer is present, this need not
                // be streaming because transformation should be fast operation.
                let mut messages = match self.transformer.as_mut() {
                    None => messages,
                    Some(transformer) => match transformer
                        .transform_batch(messages, cln_token.clone())
                        .await
                    {
                        Ok(messages) => messages,
                        Err(e) => {
                            error!(
                                ?e,
                                "Error while transforming messages, sending nack to the batch"
                            );
                            for offset in offsets {
                                self.tracker_handle
                                    .discard(offset)
                                    .await
                                    .expect("tracker operations should never fail");
                            }
                            result = Err(e);
                            break;
                        }
                    },
                };

                if let Some(watermark_handle) = self.watermark_handle.as_mut() {
                    watermark_handle
                        .generate_and_publish_source_watermark(&messages)
                        .await;

                    let watermark = watermark_handle.fetch_source_watermark().await;
                    // compare with the event time of the message and set is_late
                    for message in messages.iter_mut() {
                        message.is_late = message.event_time < watermark;
                    }
                }

                // write the messages to downstream.
                for message in messages {
                    messages_tx
                        .send(message)
                        .await
                        .expect("send should not fail");

                    processed_msgs_count += 1;
                    if last_logged_at.elapsed().as_secs() >= 1 {
                        info!(
                            "Processed {} messages in {:?}",
                            processed_msgs_count,
                            Utc::now()
                        );
                        processed_msgs_count = 0;
                        last_logged_at = Instant::now();
                    }
                }
            }
            info!(status=?result, "Source stopped, waiting for inflight messages to be acked");
            // wait for all the ack tasks to be completed before stopping the source, since we give
            // a permit for each ack task all the permits should be released when the ack tasks are
            // done, we can verify this by trying to acquire the permit for max_ack_tasks.
            let _permit = Arc::clone(&semaphore)
                .acquire_many_owned(max_ack_tasks as u32)
                .await
                .expect("acquiring permit should not fail");
            info!("All inflight messages are acked. Source stopped.");
            result
        });
        Ok((ReceiverStream::new(messages_rx), handle))
    }

    /// Listens to the oneshot receivers and invokes ack on the source for the offsets that are acked.
    async fn invoke_ack(
        e2e_start_time: Instant,
        source_handle: mpsc::Sender<ActorMessage>,
        ack_rx_batch: Vec<(Offset, oneshot::Receiver<ReadAck>)>,
        _permit: OwnedSemaphorePermit, // permit to release after acking the offsets.
        cancel_token: CancellationToken,
    ) -> Result<()> {
        let n = ack_rx_batch.len();
        let mut offsets_to_ack = Vec::with_capacity(n);

        for (offset, oneshot_rx) in ack_rx_batch {
            match oneshot_rx.await {
                Ok(ReadAck::Ack) => {
                    offsets_to_ack.push(offset);
                }
                Ok(ReadAck::Nak) => {
                    warn!(?offset, "Nak received for offset");
                }
                Err(e) => {
                    error!(?offset, err=?e, "Error receiving ack for offset");
                }
            }
        }

        let start = Instant::now();
        if !offsets_to_ack.is_empty() {
            Self::ack_with_retry(source_handle, offsets_to_ack, &cancel_token).await;
        }
        Self::send_ack_metrics(e2e_start_time, n, start);

        Ok(())
    }

    /// Invokes ack with infinite retries until the cancellation token is cancelled.
    async fn ack_with_retry(
        source_handle: mpsc::Sender<ActorMessage>,
        offsets: Vec<Offset>,
        cancel_token: &CancellationToken,
    ) {
        let interval = fixed::Interval::from_millis(ACK_RETRY_INTERVAL).take(ACK_RETRY_ATTEMPTS);
        let _ = Retry::new(
            interval,
            async || {
                let result = Self::ack(source_handle.clone(), offsets.clone()).await;
                if result.is_err() {
                    error!(?result, "Failed to send ack to source, retrying...");
                    if cancel_token.is_cancelled() {
                        error!(
                            ?result,
                            "Cancellation token received, stopping the ack retry loop"
                        );
                        return Ok(());
                    }
                }
                result
            },
            |_: &Error| !cancel_token.is_cancelled(),
        )
        .await;
    }

    fn send_read_metrics(
        pipeline_labels: &Vec<(String, String)>,
        mvtx_labels: &Vec<(String, String)>,
        read_start_time: Instant,
        n: usize,
        msgs_bytes: usize,
    ) {
        if is_mono_vertex() {
            monovertex_metrics()
                .read_total
                .get_or_create(mvtx_labels)
                .inc_by(n as u64);
            monovertex_metrics()
                .read_time
                .get_or_create(mvtx_labels)
                .observe(read_start_time.elapsed().as_micros() as f64);
            monovertex_metrics()
                .read_batch_size
                .get_or_create(mvtx_labels)
                .set(n as i64);
        } else {
            pipeline_metrics()
                .forwarder
                .read_batch_size
                .get_or_create(pipeline_labels)
                .set(n as i64);
            pipeline_metrics()
                .forwarder
                .read_total
                .get_or_create(pipeline_labels)
                .inc_by(n as u64);
            pipeline_metrics()
                .forwarder
                .data_read_total
                .get_or_create(pipeline_labels)
                .inc_by(n as u64);
            pipeline_metrics()
                .forwarder
                .read_bytes_total
                .get_or_create(pipeline_labels)
                .inc_by(msgs_bytes as u64);
            pipeline_metrics()
                .forwarder
                .data_read_bytes_total
                .get_or_create(pipeline_labels)
                .inc_by(msgs_bytes as u64);
            pipeline_metrics()
                .forwarder
                .read_processing_time
                .get_or_create(pipeline_labels)
                .observe(read_start_time.elapsed().as_micros() as f64);
        }
    }

    fn send_ack_metrics(e2e_start_time: Instant, n: usize, start: Instant) {
        if is_mono_vertex() {
            let mvtx_labels = mvtx_forward_metric_labels();
            monovertex_metrics()
                .ack_time
                .get_or_create(mvtx_labels)
                .observe(start.elapsed().as_micros() as f64);

            monovertex_metrics()
                .ack_total
                .get_or_create(mvtx_labels)
                .inc_by(n as u64);

            monovertex_metrics()
                .e2e_time
                .get_or_create(mvtx_labels)
                .observe(e2e_start_time.elapsed().as_micros() as f64);
        } else {
            let mut pipeline_labels = pipeline_metric_labels(VERTEX_TYPE_SOURCE).clone();
            pipeline_labels.push((
                PIPELINE_PARTITION_NAME_LABEL.to_string(),
                get_vertex_name().to_string(),
            ));
            pipeline_metrics()
                .forwarder
                .ack_processing_time
                .get_or_create(&pipeline_labels)
                .observe(start.elapsed().as_micros() as f64);

            pipeline_metrics()
                .forwarder
                .ack_total
                .get_or_create(&pipeline_labels)
                .inc_by(n as u64);
            pipeline_metrics()
                .forwarder
                .e2e_time
                .get_or_create(&pipeline_labels)
                .observe(e2e_start_time.elapsed().as_micros() as f64);
        }
    }

    pub(crate) async fn ready(&mut self) -> bool {
        let source_ready = if let Some(client) = &mut self.health_checker {
            let request = tonic::Request::new(());
            match client.is_ready(request).await {
                Ok(response) => response.into_inner().ready,
                Err(e) => {
                    error!("Source is not ready: {:?}", e);
                    false
                }
            }
        } else {
            true
        };

        let transformer_ready = if let Some(client) = &mut self.transformer {
            client.ready().await
        } else {
            true
        };

        source_ready && transformer_ready
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use numaflow::source;
    use numaflow::source::{Message, Offset, SourceReadRequest};
    use numaflow_pb::clients::source::source_client::SourceClient;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::sync::mpsc::Sender;
    use tokio_stream::StreamExt;
    use tokio_util::sync::CancellationToken;

    use crate::shared::grpc::create_rpc_channel;
    use crate::source::user_defined::new_source;
    use crate::source::{Source, SourceType};
    use crate::tracker::TrackerHandle;

    struct SimpleSource {
        num: usize,
        sent_count: AtomicUsize,
        yet_to_ack: std::sync::RwLock<HashSet<String>>,
    }

    impl SimpleSource {
        fn new(num: usize) -> Self {
            Self {
                num,
                sent_count: AtomicUsize::new(0),
                yet_to_ack: std::sync::RwLock::new(HashSet::new()),
            }
        }
    }

    #[tonic::async_trait]
    impl source::Sourcer for SimpleSource {
        async fn read(&self, request: SourceReadRequest, transmitter: Sender<Message>) {
            let event_time = Utc::now();
            let mut message_offsets = Vec::with_capacity(request.count);

            for i in 0..request.count {
                if self.sent_count.load(Ordering::SeqCst) >= self.num {
                    return;
                }

                let offset = format!("{}-{}", event_time.timestamp_nanos_opt().unwrap(), i);
                transmitter
                    .send(Message {
                        value: b"hello".to_vec(),
                        event_time,
                        offset: Offset {
                            offset: offset.clone().into_bytes(),
                            partition_id: 0,
                        },
                        keys: vec![],
                        headers: Default::default(),
                    })
                    .await
                    .unwrap();
                message_offsets.push(offset);
                self.sent_count.fetch_add(1, Ordering::SeqCst);
            }
            self.yet_to_ack.write().unwrap().extend(message_offsets);
        }

        async fn ack(&self, offsets: Vec<Offset>) {
            for offset in offsets {
                self.yet_to_ack
                    .write()
                    .unwrap()
                    .remove(&String::from_utf8(offset.offset).unwrap());
            }
        }

        async fn pending(&self) -> Option<usize> {
            Some(self.yet_to_ack.read().unwrap().len())
        }

        async fn partitions(&self) -> Option<Vec<i32>> {
            Some(vec![1, 2])
        }
    }

    #[tokio::test]
    async fn test_source() {
        // start the server
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let sock_file = tmp_dir.path().join("source.sock");
        let server_info_file = tmp_dir.path().join("source-server-info");

        let server_info = server_info_file.clone();
        let server_socket = sock_file.clone();
        let server_handle = tokio::spawn(async move {
            // a simple source which generates total of 100 messages
            source::Server::new(SimpleSource::new(100))
                .with_socket_file(server_socket)
                .with_server_info_file(server_info)
                .start_with_shutdown(shutdown_rx)
                .await
                .unwrap()
        });

        // wait for the server to start
        // TODO: flaky
        tokio::time::sleep(Duration::from_millis(100)).await;

        let client = SourceClient::new(create_rpc_channel(sock_file).await.unwrap());

        let (src_read, src_ack, lag_reader) = new_source(client, 5, Duration::from_millis(1000))
            .await
            .map_err(|e| panic!("failed to create source reader: {:?}", e))
            .unwrap();

        let tracker = TrackerHandle::new(None);
        let source = Source::new(
            5,
            SourceType::UserDefinedSource(Box::new(src_read), Box::new(src_ack), lag_reader),
            tracker.clone(),
            true,
            None,
            None,
        );

        let sender = source.sender.clone();

        let cln_token = CancellationToken::new();

        let (mut stream, handle) = source.clone().streaming_read(cln_token.clone()).unwrap();
        let mut offsets = vec![];
        // we should read all the 100 messages
        for _ in 0..100 {
            let message = stream.next().await.unwrap();
            assert_eq!(message.value, "hello".as_bytes());
            offsets.push(message.offset.clone());
        }

        // ack all the messages
        Source::ack(sender.clone(), offsets.clone()).await.unwrap();

        for offset in offsets {
            tracker.discard(offset).await.unwrap();
        }

        // since we acked all the messages, pending should be 0
        let pending = source.pending().await.unwrap();
        assert_eq!(pending, Some(0));

        let partitions = Source::partitions(sender.clone()).await.unwrap();
        assert_eq!(partitions, vec![1, 2]);

        drop(source);
        drop(sender);

        cln_token.cancel();
        let _ = handle.await.unwrap();
        let _ = shutdown_tx.send(());
        server_handle.await.unwrap();
    }
}
