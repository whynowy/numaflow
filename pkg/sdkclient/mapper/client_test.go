/*
Copyright 2022 The Numaproj Authors.

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
*/

package mapper

import (
	"context"
	"errors"
	"fmt"
	"net"
	"testing"
	"time"

	mappb "github.com/numaproj/numaflow-go/pkg/apis/proto/map/v1"
	"github.com/numaproj/numaflow-go/pkg/mapper"
	"github.com/stretchr/testify/require"
	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
	"google.golang.org/grpc/test/bufconn"
	"google.golang.org/protobuf/types/known/emptypb"
)

func TestClient_IsReady(t *testing.T) {
	var ctx = context.Background()
	svc := &mapper.Service{
		Mapper: mapper.MapperFunc(func(ctx context.Context, keys []string, datum mapper.Datum) mapper.Messages {
			return mapper.MessagesBuilder()
		}),
	}

	// Start the gRPC server
	conn := newServer(t, func(server *grpc.Server) {
		mappb.RegisterMapServer(server, svc)
	})
	defer func(conn *grpc.ClientConn) {
		_ = conn.Close()
	}(conn)

	// Create a client connection to the server
	client := mappb.NewMapClient(conn)

	testClient, err := NewFromClient(ctx, client)
	require.NoError(t, err)

	ready, err := testClient.IsReady(ctx, &emptypb.Empty{})
	require.True(t, ready)
	require.NoError(t, err)
}

func newServer(t *testing.T, register func(server *grpc.Server)) *grpc.ClientConn {
	lis := bufconn.Listen(100)
	t.Cleanup(func() {
		_ = lis.Close()
	})

	server := grpc.NewServer()
	t.Cleanup(func() {
		server.Stop()
	})

	register(server)

	errChan := make(chan error, 1)
	go func() {
		// t.Fatal should only be called from the goroutine running the test
		if err := server.Serve(lis); err != nil {
			errChan <- err
		}
	}()

	dialer := func(context.Context, string) (net.Conn, error) {
		return lis.Dial()
	}

	conn, err := grpc.NewClient("passthrough://", grpc.WithContextDialer(dialer), grpc.WithTransportCredentials(insecure.NewCredentials()))
	t.Cleanup(func() {
		_ = conn.Close()
	})
	if err != nil {
		t.Fatalf("Creating new gRPC client connection: %v", err)
	}

	var grpcServerErr error
	select {
	case grpcServerErr = <-errChan:
	case <-time.After(500 * time.Millisecond):
		grpcServerErr = errors.New("gRPC server didn't start in 500ms")
	}
	if err != nil {
		t.Fatalf("Failed to start gRPC server: %v", grpcServerErr)
	}

	return conn
}

func TestClient_MapFn(t *testing.T) {
	svc := &mapper.Service{
		Mapper: mapper.MapperFunc(func(ctx context.Context, keys []string, datum mapper.Datum) mapper.Messages {
			msg := datum.Value()
			return mapper.MessagesBuilder().Append(mapper.NewMessage(msg).WithKeys([]string{keys[0] + "_test"}))
		}),
	}
	conn := newServer(t, func(server *grpc.Server) {
		mappb.RegisterMapServer(server, svc)
	})
	mapClient := mappb.NewMapClient(conn)
	var ctx = context.Background()
	client, _ := NewFromClient(ctx, mapClient)

	requests := make([]*mappb.MapRequest, 5)
	for i := 0; i < 5; i++ {
		requests[i] = &mappb.MapRequest{
			Request: &mappb.MapRequest_Request{
				Keys:  []string{fmt.Sprintf("client_key_%d", i)},
				Value: []byte("test"),
			},
		}
	}

	responses, err := client.MapFn(ctx, requests)
	require.NoError(t, err)
	var results [][]*mappb.MapResponse_Result
	for _, resp := range responses {
		results = append(results, resp.GetResults())
	}
	expected := [][]*mappb.MapResponse_Result{
		{{Keys: []string{"client_key_0_test"}, Value: []byte("test")}},
		{{Keys: []string{"client_key_1_test"}, Value: []byte("test")}},
		{{Keys: []string{"client_key_2_test"}, Value: []byte("test")}},
		{{Keys: []string{"client_key_3_test"}, Value: []byte("test")}},
		{{Keys: []string{"client_key_4_test"}, Value: []byte("test")}},
	}
	require.ElementsMatch(t, expected, results)
}
