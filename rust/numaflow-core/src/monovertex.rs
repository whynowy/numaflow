use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::config::is_mono_vertex;
use crate::config::monovertex::MonovertexConfig;
use crate::error::{self};
use crate::metrics::{LagReader, PendingReaderTasks};
use crate::shared::create_components;
use crate::sink::SinkWriter;
use crate::source::Source;
use crate::tracker::TrackerHandle;
use crate::{metrics, shared};

/// [forwarder] orchestrates data movement from the Source to the Sink via the optional SourceTransformer.
/// The forward-a-chunk executes the following in an infinite loop till a shutdown signal is received:
/// - Read X messages from the source
/// - Invokes the SourceTransformer concurrently
/// - Calls the Sinker to write the batch to the Sink
/// - Send Acknowledgement back to the Source
pub(crate) mod forwarder;

pub(crate) async fn start_forwarder(
    cln_token: CancellationToken,
    config: &MonovertexConfig,
) -> error::Result<()> {
    let tracker_handle = TrackerHandle::new(None);

    let transformer = create_components::create_transformer(
        config.batch_size,
        config.graceful_shutdown_time,
        config.transformer_config.clone(),
        tracker_handle.clone(),
        cln_token.clone(),
    )
    .await?;

    let source = create_components::create_source(
        config.batch_size,
        config.read_timeout,
        &config.source_config,
        tracker_handle.clone(),
        transformer,
        None,
        cln_token.clone(),
    )
    .await?;

    let sink_writer = create_components::create_sink_writer(
        config.batch_size,
        config.read_timeout,
        config.sink_config.clone(),
        config.fb_sink_config.clone(),
        tracker_handle,
        None,
        &cln_token,
    )
    .await?;

    // Start the metrics server in a separate background async spawn,
    // This should be running throughout the lifetime of the application, hence the handle is not
    // joined.
    let metrics_state = metrics::MetricsState {
        health_checks: metrics::ComponentHealthChecks::Monovertex(Box::new(
            metrics::MonovertexComponents {
                source: source.clone(),
                sink: sink_writer.clone(),
            },
        )),
        watermark_fetcher_state: None, // Monovertex doesn't have watermark handles
    };

    // start the metrics server
    // FIXME: what to do with the handle
    let metrics_server_handle =
        shared::metrics::start_metrics_server(config.metrics_config.clone(), metrics_state).await;

    start(config.clone(), source, sink_writer, cln_token).await?;

    // abort the metrics server
    metrics_server_handle.abort();
    Ok(())
}

async fn start(
    mvtx_config: MonovertexConfig,
    source: Source,
    sink: SinkWriter,
    cln_token: CancellationToken,
) -> error::Result<()> {
    // Store the pending reader handle outside, so it doesn't get dropped immediately.

    // only check the pending and lag for source for pod_id = 0
    let _pending_reader_handle: Option<PendingReaderTasks> = if mvtx_config.replica == 0 {
        // start the pending reader to publish pending metrics
        let pending_reader = shared::metrics::create_pending_reader(
            &mvtx_config.metrics_config,
            LagReader::Source(Box::new(source.clone())),
        )
        .await;
        Some(pending_reader.start(is_mono_vertex()).await)
    } else {
        None
    };

    let forwarder = forwarder::Forwarder::new(source, sink);

    info!("Forwarder is starting...");
    // start the forwarder, it will return only on Signal
    forwarder.start(cln_token).await?;

    info!("Forwarder stopped gracefully.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use numaflow::source::{Message, Offset, SourceReadRequest};
    use numaflow::{sink, source};
    use tokio::sync::mpsc::Sender;
    use tokio_util::sync::CancellationToken;

    use crate::config::components;
    use crate::config::monovertex::MonovertexConfig;
    use crate::monovertex::start_forwarder;

    struct SimpleSource;
    #[tonic::async_trait]
    impl source::Sourcer for SimpleSource {
        async fn read(&self, _: SourceReadRequest, _: Sender<Message>) {}

        async fn ack(&self, _: Vec<Offset>) {}

        async fn pending(&self) -> Option<usize> {
            Some(0)
        }

        async fn partitions(&self) -> Option<Vec<i32>> {
            None
        }
    }

    struct SimpleSink;

    #[tonic::async_trait]
    impl sink::Sinker for SimpleSink {
        async fn sink(
            &self,
            _input: tokio::sync::mpsc::Receiver<sink::SinkRequest>,
        ) -> Vec<sink::Response> {
            vec![]
        }
    }

    #[tokio::test]
    async fn run_forwarder() {
        let (src_shutdown_tx, src_shutdown_rx) = tokio::sync::oneshot::channel();
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let src_sock_file = tmp_dir.path().join("source.sock");
        let src_info_file = tmp_dir.path().join("sourcer-server-info");

        let server_info = src_info_file.clone();
        let server_socket = src_sock_file.clone();
        let src_server_handle = tokio::spawn(async move {
            source::Server::new(SimpleSource)
                .with_socket_file(server_socket)
                .with_server_info_file(server_info)
                .start_with_shutdown(src_shutdown_rx)
                .await
                .unwrap();
        });

        let (sink_shutdown_tx, sink_shutdown_rx) = tokio::sync::oneshot::channel();
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let sink_sock_file = tmp_dir.path().join("sink.sock");
        let sink_info_file = tmp_dir.path().join("sinker-server-info");

        let server_socket = sink_sock_file.clone();
        let server_info = sink_info_file.clone();
        let sink_server_handle = tokio::spawn(async move {
            sink::Server::new(SimpleSink)
                .with_socket_file(server_socket)
                .with_server_info_file(server_info)
                .start_with_shutdown(sink_shutdown_rx)
                .await
                .unwrap();
        });

        // wait for the servers to start
        // FIXME: we need to have a better way, this is flaky
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let cln_token = CancellationToken::new();

        let token_clone = cln_token.clone();
        tokio::spawn(async move {
            // FIXME: we need to have a better way, this is flaky
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            token_clone.cancel();
        });

        let config = MonovertexConfig {
            source_config: components::source::SourceConfig {
                read_ahead: false,
                source_type: components::source::SourceType::UserDefined(
                    components::source::UserDefinedConfig {
                        socket_path: src_sock_file.to_str().unwrap().to_string(),
                        grpc_max_message_size: 1024,
                        server_info_path: src_info_file.to_str().unwrap().to_string(),
                    },
                ),
            },
            sink_config: components::sink::SinkConfig {
                sink_type: components::sink::SinkType::UserDefined(
                    components::sink::UserDefinedConfig {
                        socket_path: sink_sock_file.to_str().unwrap().to_string(),
                        grpc_max_message_size: 1024,
                        server_info_path: sink_info_file.to_str().unwrap().to_string(),
                    },
                ),
                retry_config: Default::default(),
            },
            ..Default::default()
        };

        let result = start_forwarder(cln_token.clone(), &config).await;
        assert!(result.is_ok());

        // stop the source and sink servers
        src_shutdown_tx.send(()).unwrap();
        sink_shutdown_tx.send(()).unwrap();

        src_server_handle.await.unwrap();
        sink_server_handle.await.unwrap();
    }
}
