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

package runtime

import (
	"context"
	"crypto/tls"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"sync"
	"time"

	"go.uber.org/zap"

	"github.com/numaproj/numaflow/pkg/apis/numaflow/v1alpha1"
	"github.com/numaproj/numaflow/pkg/shared/logging"
)

const (
	runtimeErrorsPath     = "runtime/errors"
	runtimeErrorsTimeStep = 60 * time.Second
)

// ErrorDetails is used to provide information for a container error
// including timestamp, error code, message and details
type ErrorDetails struct {
	Container string `json:"container"`
	Timestamp int64  `json:"timestamp"`
	Code      string `json:"code"`
	Message   string `json:"message"`
	Details   string `json:"details"`
}

type ErrorApiResponse struct {
	ErrMessage string         `json:"errorMessage"`
	Data       []ErrorDetails `json:"data"`
}

// ReplicaErrors is used to provide all the container errors for a replica
type ReplicaErrors struct {
	Replica         string         `json:"replica"`
	ContainerErrors []ErrorDetails `json:"containerErrors"`
}

// PipelineRuntimeCache is an interface for caching and retrieving the runtime information.
type PipelineRuntimeCache interface {
	// StartCacheRefresher starts the cache refresher to update the local cache with the runtime errors.
	StartCacheRefresher(ctx context.Context) error
	// GetLocalCache returns the local cache of runtime errors for each vertex.
	GetLocalCache() map[string][]ReplicaErrors
}

var _ PipelineRuntimeCache = (*pipelineRuntimeCache)(nil)

type monitorHttpClient interface {
	Get(url string) (*http.Response, error)
	Head(url string) (*http.Response, error)
}

// pipelineRuntimeCache is used to store the local cache of runtime errors for a vertex.
// It implements the PipelineRuntimeCache interface.
type pipelineRuntimeCache struct {
	pipeline   *v1alpha1.Pipeline
	localCache map[string][]ReplicaErrors
	cacheMutex sync.RWMutex
	podTracker *PodTracker
	log        *zap.SugaredLogger
	httpClient monitorHttpClient
}

func NewRuntime(ctx context.Context, pl *v1alpha1.Pipeline) PipelineRuntimeCache {
	return &pipelineRuntimeCache{
		pipeline:   pl,
		localCache: make(map[string][]ReplicaErrors),
		cacheMutex: sync.RWMutex{},
		log:        logging.FromContext(ctx).Named("pipelineRuntimeCache"),
		httpClient: &http.Client{
			Transport: &http.Transport{
				TLSClientConfig: &tls.Config{InsecureSkipVerify: true},
			},
			Timeout: time.Second * 1,
		},
		podTracker: NewPodTracker(ctx, pl),
	}
}

// StartCacheRefresher starts the cache refresher to update the local cache periodically with the runtime errors.
func (r *pipelineRuntimeCache) StartCacheRefresher(ctx context.Context) (err error) {
	r.log.Infof("Starting runtime cache refresher...")

	ctx, cancel := context.WithCancel(logging.WithLogger(ctx, r.log))

	go func() {
		err := r.podTracker.Start(ctx)
		if err != nil {
			r.log.Errorw("Failed to start pod tracker for runtime cache refresher", zap.Error(err))
			cancel()
		}
	}()

	// wait for first active pods update for vertices
	<-r.podTracker.firstPodsUpdateChan
	// start persisting errors into the local cache
	go r.persistRuntimeErrors(ctx)

	return nil
}

// persistRuntimeErrors updates the local cache with the runtime errors
func (r *pipelineRuntimeCache) persistRuntimeErrors(ctx context.Context) {
	// Define a function to fetch and persist errors
	fetchAndPersistErrors := func() {
		var wg sync.WaitGroup
		for _, vtx := range r.pipeline.Spec.Vertices {
			for i := range r.podTracker.GetActivePodsCountForVertex(vtx.Name) {
				wg.Add(1)
				go func(vtxName string, podIndex int) {
					defer wg.Done()
					r.fetchAndPersistErrorForPod(vtxName, podIndex)
				}(vtx.Name, i)
			}
		}
		wg.Wait()
	}

	// invoke once and then periodically update the cache
	fetchAndPersistErrors()
	// Set up a ticker to run periodically
	ticker := time.NewTicker(runtimeErrorsTimeStep)
	defer ticker.Stop()
	for {
		select {
		case <-ticker.C:
			fetchAndPersistErrors()
		// If the context is done, return.
		case <-ctx.Done():
			r.log.Info("Context canceled, stopping PersistRuntimeErrors")
			return
		}
	}
}

// fetchAndPersistErrorForPod fetches the runtime errors for a pod and persists them in the local cache.
func (r *pipelineRuntimeCache) fetchAndPersistErrorForPod(vtxName string, podIndex int) {
	podName := strings.Join([]string{r.pipeline.Name, vtxName, fmt.Sprintf("%d", podIndex)}, "-")
	// Get the headless service name
	url := fmt.Sprintf("https://%s.%s.%s.svc:%v/%s", podName, r.pipeline.Name+"-"+vtxName+"-headless", r.pipeline.Namespace, v1alpha1.VertexMonitorPort, runtimeErrorsPath)

	res, err := r.httpClient.Get(url)
	if err != nil {
		r.log.Warnf("[vertex name %s, pod name %s]: failed reading the runtime endpoint, the pod might have been scaled down: %v", vtxName, podName, err.Error())
		return
	}

	// Read the response body
	body, err := io.ReadAll(res.Body)
	_ = res.Body.Close()
	if err != nil {
		r.log.Errorf("Error reading response body from %s: %v", url, err)
		return
	}

	// Parse the response body into runtime api response
	var apiResponse ErrorApiResponse
	if err := json.Unmarshal(body, &apiResponse); err != nil {
		r.log.Errorf("Error decoding runtime error response from %s: %v", url, err)
		return
	}

	// return if data array length is 0 or if there is any error in the API call
	if apiResponse.ErrMessage != "" || len(apiResponse.Data) == 0 {
		return
	}

	cacheKey := fmt.Sprintf("%s-%s", r.pipeline.Name, vtxName)
	replica := fmt.Sprintf("%s-%s-%v", r.pipeline.Name, vtxName, podIndex)
	newVtxErrors := ReplicaErrors{Replica: replica, ContainerErrors: apiResponse.Data}

	r.log.Debugf("Persisting error in local cache for: %s", cacheKey)
	// Lock the cache before updating
	r.cacheMutex.Lock()
	defer r.cacheMutex.Unlock()

	// overwrite the errors
	if vtxErrors, ok := r.localCache[cacheKey]; ok {
		// Update existing replica errors if found
		for i, vtxError := range vtxErrors {
			if vtxError.Replica == replica {
				vtxErrors[i] = newVtxErrors
				r.localCache[cacheKey] = vtxErrors
				return
			}
		}
		// Append new replica errors if not found
		r.localCache[cacheKey] = append(vtxErrors, newVtxErrors)
	} else {
		// Initialize the cache entry with the new replica errors
		r.localCache[cacheKey] = []ReplicaErrors{newVtxErrors}
	}
}

// GetLocalCache returns the local cache of runtime errors.
func (r *pipelineRuntimeCache) GetLocalCache() map[string][]ReplicaErrors {
	r.cacheMutex.RLock()
	defer r.cacheMutex.RUnlock()

	localCacheCopy := make(map[string][]ReplicaErrors, len(r.localCache))
	for key, value := range r.localCache {
		localCacheValue := make([]ReplicaErrors, len(value))
		copy(localCacheValue, value)
		localCacheCopy[key] = localCacheValue
	}
	return localCacheCopy
}
