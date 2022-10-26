package udf

import (
	"context"
	"fmt"
	"sync"
	"time"

	dfv1 "github.com/numaproj/numaflow/pkg/apis/numaflow/v1alpha1"
	"github.com/numaproj/numaflow/pkg/isb"
	"github.com/numaproj/numaflow/pkg/isb/forward"
	"github.com/numaproj/numaflow/pkg/metrics"
	"github.com/numaproj/numaflow/pkg/shared/logging"
	sharedutil "github.com/numaproj/numaflow/pkg/shared/util"
	"github.com/numaproj/numaflow/pkg/watermark/generic/jetstream"

	"github.com/numaproj/numaflow/pkg/udf/function"
	"github.com/numaproj/numaflow/pkg/watermark/generic"
	"go.uber.org/zap"
)

type MapUDFProcessor struct {
	ISBSvcType     dfv1.ISBSvcType
	VertexInstance *dfv1.VertexInstance
}

func (u *MapUDFProcessor) Start(ctx context.Context) error {
	log := logging.FromContext(ctx)
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	var reader isb.BufferReader
	var err error
	fromBufferName := u.VertexInstance.Vertex.GetFromBuffers()[0].Name
	toBuffers := u.VertexInstance.Vertex.GetToBuffers()
	writers := make(map[string]isb.BufferWriter)

	// watermark variables
	fetchWatermark, publishWatermark := generic.BuildNoOpWatermarkProgressorsFromEdgeList(generic.GetBufferNameList(toBuffers))

	switch u.ISBSvcType {
	case dfv1.ISBSvcTypeRedis:
		reader, writers = buildRedisBufferIO(ctx, fromBufferName, u.VertexInstance)
	case dfv1.ISBSvcTypeJetStream:
		fromStreamName := isbsvc.JetStreamName(u.VertexInstance.Vertex.Spec.PipelineName, fromBufferName)
		readOptions := []jetstreamisb.ReadOption{
			jetstreamisb.WithUsingAckInfoAsRate(true),
		}
		if x := u.VertexInstance.Vertex.Spec.Limits; x != nil && x.ReadTimeout != nil {
			readOptions = append(readOptions, jetstreamisb.WithReadTimeOut(x.ReadTimeout.Duration))
		}
		reader, err = jetstreamisb.NewJetStreamBufferReader(ctx, jsclient.NewInClusterJetStreamClient(), fromBufferName, fromStreamName, fromStreamName, readOptions...)
		if err != nil {
			return err
		}
		reader, writers, err = buildJetStreamBufferIO(ctx, fromBufferName, u.VertexInstance)
		if err != nil {
			return err
		}

	default:
		return fmt.Errorf("unrecognized isbsvc type %q", u.ISBSvcType)
	}

	// TODO update this to support shuffle
	conditionalForwarder := forward.GoWhere(func(key string) ([]string, error) {
		result := []string{}
		_key := string(key)
		if _key == dfv1.MessageKeyAll || _key == dfv1.MessageKeyDrop {
			result = append(result, _key)
			return result, nil
		}
		for _, to := range u.VertexInstance.Vertex.Spec.ToEdges {
			// If returned key is not "ALL" or "DROP", and there's no conditions defined in the edge,
			// treat it as "ALL"?
			if to.Conditions == nil || len(to.Conditions.KeyIn) == 0 || sharedutil.StringSliceContains(to.Conditions.KeyIn, _key) {
				result = append(result, dfv1.GenerateEdgeBufferNames(u.VertexInstance.Vertex.Namespace, u.VertexInstance.Vertex.Spec.PipelineName, to)...)
			}
		}
		return result, nil
	})

	log = log.With("protocol", "uds-grpc-map-udf")
	udfHandler, err := function.NewUDSGRPCBasedUDF()
	if err != nil {
		return fmt.Errorf("failed to create gRPC client, %w", err)
	}
	// Readiness check
	if err := udfHandler.WaitUntilReady(ctx); err != nil {
		return fmt.Errorf("failed on UDF readiness check, %w", err)
	}
	defer func() {
		err = udfHandler.CloseConn(ctx)
		if err != nil {
			log.Warnw("Failed to close gRPC client conn", zap.Error(err))
		}
	}()
	log.Infow("Start processing udf messages", zap.String("isbsvc", string(u.ISBSvcType)), zap.String("from", fromBufferName), zap.Any("to", toBuffers))

	opts := []forward.Option{forward.WithVertexType(dfv1.VertexTypeMapUDF), forward.WithLogger(log)}
	if x := u.VertexInstance.Vertex.Spec.Limits; x != nil {
		if x.ReadBatchSize != nil {
			opts = append(opts, forward.WithReadBatchSize(int64(*x.ReadBatchSize)))
			opts = append(opts, forward.WithUDFConcurrency(int(*x.ReadBatchSize)))
		}
	}

	forwarder, err := forward.NewInterStepDataForward(u.VertexInstance.Vertex, reader, writers, conditionalForwarder, udfHandler, fetchWatermark, publishWatermark, opts...)
	if err != nil {
		return err
	}
	stopped := forwarder.Start()

	wg := &sync.WaitGroup{}
	wg.Add(1)
	go func() {
		defer wg.Done()
		for {
			<-stopped
			log.Info("Forwarder stopped, exiting udf map data processor...")
			return
		}
	}()

	metricsOpts := []metrics.Option{
		metrics.WithLookbackSeconds(int64(u.VertexInstance.Vertex.Spec.Scale.GetLookbackSeconds())),
		metrics.WithHealthCheckExecutor(func() error {
			cctx, cancel := context.WithTimeout(ctx, 20*time.Second)
			defer cancel()
			return udfHandler.WaitUntilReady(cctx)
		}),
	}
	if x, ok := reader.(isb.LagReader); ok {
		metricsOpts = append(metricsOpts, metrics.WithLagReader(x))
	}
	if x, ok := reader.(isb.Ratable); ok {
		metricsOpts = append(metricsOpts, metrics.WithRater(x))
	}
	ms := metrics.NewMetricsServer(u.VertexInstance.Vertex, metricsOpts...)
	if shutdown, err := ms.Start(ctx); err != nil {
		return fmt.Errorf("failed to start metrics server, error: %w", err)
	} else {
		defer func() { _ = shutdown(context.Background()) }()
	}

	<-ctx.Done()
	log.Info("SIGTERM, exiting...")
	forwarder.Stop()
	wg.Wait()
	log.Info("Exited...")
	return nil
}
