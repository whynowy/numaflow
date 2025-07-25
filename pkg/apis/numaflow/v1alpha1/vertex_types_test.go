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

package v1alpha1

import (
	"fmt"
	"strings"
	"testing"

	"github.com/stretchr/testify/assert"
	corev1 "k8s.io/api/core/v1"
	resource "k8s.io/apimachinery/pkg/api/resource"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/utils/ptr"
)

const (
	testNamespace      = "test-ns"
	testVertexSpecName = "vtx"
	testPipelineName   = "test-pl"
	testVertexName     = testPipelineName + "-" + testVertexSpecName
	testFlowImage      = "test-f-image"
)

var (
	testReplicas  = int32(1)
	testSrcVertex = &Vertex{
		ObjectMeta: metav1.ObjectMeta{
			Namespace: testNamespace,
			Name:      testVertexName,
		},
		Spec: VertexSpec{
			Replicas:     &testReplicas,
			PipelineName: testPipelineName,
			AbstractVertex: AbstractVertex{
				Name:   testVertexSpecName,
				Source: &Source{},
			},
			ToEdges: []CombinedEdge{{Edge: Edge{From: testVertexSpecName, To: "output"}}},
		},
	}

	testSinkVertex = &Vertex{
		ObjectMeta: metav1.ObjectMeta{
			Namespace: testNamespace,
			Name:      testVertexName,
		},
		Spec: VertexSpec{
			Replicas:     &testReplicas,
			PipelineName: testPipelineName,
			AbstractVertex: AbstractVertex{
				Name: testVertexSpecName,
				Sink: &Sink{},
			},
			FromEdges: []CombinedEdge{{Edge: Edge{From: "input", To: testVertexSpecName}}},
		},
	}

	testVertex = &Vertex{
		ObjectMeta: metav1.ObjectMeta{
			Namespace: testNamespace,
			Name:      testVertexName,
		},
		Spec: VertexSpec{
			Replicas:     &testReplicas,
			PipelineName: testPipelineName,
			AbstractVertex: AbstractVertex{
				Name: testVertexSpecName,
			},
			FromEdges: []CombinedEdge{{Edge: Edge{From: "input", To: testVertexSpecName}}},
			ToEdges:   []CombinedEdge{{Edge: Edge{From: testVertexSpecName, To: "output"}}},
		},
	}
)

func TestOwnedBuffers(t *testing.T) {
	f := testVertex.OwnedBuffers()
	assert.Equal(t, 1, len(f))
	assert.Equal(t, f[0], fmt.Sprintf("%s-%s-%s-0", testVertex.Namespace, testVertex.Spec.PipelineName, testVertex.Spec.Name))
}

func TestOwnedBuffersSource(t *testing.T) {
	f := testSrcVertex.OwnedBuffers()
	assert.Equal(t, 0, len(f))
}

func TestGetFromBuckets(t *testing.T) {
	f := testVertex.GetFromBuckets()
	assert.Equal(t, 1, len(f))
	assert.Equal(t, f[0], fmt.Sprintf("%s-%s-%s-%s", testVertex.Namespace, testVertex.Spec.PipelineName, "input", testVertex.Spec.Name))
	f = testSrcVertex.GetFromBuckets()
	assert.Equal(t, 1, len(f))
	assert.Equal(t, f[0], fmt.Sprintf("%s-%s-%s_SOURCE", testVertex.Namespace, testVertex.Spec.PipelineName, testVertex.Spec.Name))
}

func TestGetToBuffers(t *testing.T) {
	f := testVertex.GetToBuffers()
	assert.Equal(t, 1, len(f))
	assert.Contains(t, f[0], fmt.Sprintf("%s-%s-%s-0", testVertex.Namespace, testVertex.Spec.PipelineName, "output"))
}

func TestGetToBuffersSink(t *testing.T) {
	f := testSinkVertex.GetToBuffers()
	assert.Equal(t, 0, len(f))
}

func TestWithoutReplicas(t *testing.T) {
	s := &VertexSpec{
		Replicas: ptr.To[int32](3),
		Lifecycle: VertexLifecycle{
			DesiredPhase: VertexPhasePaused,
		},
	}
	dc := s.DeepCopyWithoutReplicasAndLifecycle()
	assert.Equal(t, int32(0), *dc.Replicas)
	assert.Equal(t, VertexLifecycle{}, dc.Lifecycle)
}

func TestGetVertexReplicas(t *testing.T) {
	v := Vertex{
		Spec: VertexSpec{
			AbstractVertex: AbstractVertex{
				Name: "b",
			},
		},
	}
	v.Spec.Lifecycle.DesiredPhase = VertexPhasePaused
	assert.Equal(t, 0, v.CalculateReplicas())
	v.Spec.Lifecycle.DesiredPhase = VertexPhaseRunning
	assert.Equal(t, 1, v.CalculateReplicas())
	v.Spec.Lifecycle = VertexLifecycle{}
	assert.Equal(t, 1, v.CalculateReplicas())
	v.Spec.Replicas = ptr.To[int32](3)
	assert.Equal(t, 3, v.CalculateReplicas())
	v.Spec.Replicas = ptr.To[int32](0)
	assert.Equal(t, 0, v.CalculateReplicas())
	v.Spec.UDF = &UDF{
		GroupBy: &GroupBy{},
	}
	v.Spec.FromEdges = []CombinedEdge{
		{Edge: Edge{From: "a", To: "b"}},
	}
	v.Spec.Replicas = ptr.To[int32](5)
	assert.Equal(t, 1, v.CalculateReplicas())
	v.Spec.Replicas = ptr.To[int32](1000)
	assert.Equal(t, 1, v.CalculateReplicas())
	v.Spec.UDF.GroupBy = nil
	v.Spec.Scale.Max = ptr.To[int32](40)
	v.Spec.Scale.Min = ptr.To[int32](20)
	v.Spec.Replicas = ptr.To[int32](300)
	assert.Equal(t, 40, v.CalculateReplicas())
	v.Spec.Replicas = ptr.To[int32](10)
	assert.Equal(t, 20, v.CalculateReplicas())
}

func TestGetHeadlessSvcSpec(t *testing.T) {
	s := testVertex.getServiceObj(testVertex.GetHeadlessServiceName(), true, map[string]int32{VertexMetricsPortName: VertexMetricsPort})
	assert.Equal(t, s.Name, testVertex.GetHeadlessServiceName())
	assert.Equal(t, s.Namespace, testVertex.Namespace)
	assert.Equal(t, 1, len(s.Spec.Ports))
	assert.Equal(t, "None", s.Spec.ClusterIP)
}

func TestGetServiceObjs(t *testing.T) {
	s := testVertex.GetServiceObjs()
	assert.Equal(t, 1, len(s))

	v := testVertex.DeepCopy()
	v.Spec.UDF = nil
	v.Spec.Source = &Source{
		HTTP: &HTTPSource{},
	}
	s = v.GetServiceObjs()
	assert.Equal(t, 1, len(s))
	assert.Equal(t, s[0].Name, v.GetHeadlessServiceName())
	assert.Equal(t, 2, len(s[0].Spec.Ports))
	ports := map[int32]bool{
		VertexMetricsPort: false,
		VertexMonitorPort: false,
	}
	for _, port := range s[0].Spec.Ports {
		ports[port.Port] = true
	}
	assert.True(t, ports[VertexMetricsPort], "Metrics port is missing")
	assert.True(t, ports[VertexMonitorPort], "Monitor port is missing")
	assert.Equal(t, "None", s[0].Spec.ClusterIP)

	v.Spec.Source.HTTP.Service = true
	s = v.GetServiceObjs()
	assert.Equal(t, 2, len(s))
	assert.Equal(t, s[1].Name, v.Name)
	assert.Equal(t, 1, len(s[1].Spec.Ports))
	assert.Equal(t, VertexHTTPSPort, int(s[1].Spec.Ports[0].Port))
}

func TestGetHeadlessServiceName(t *testing.T) {
	n := testVertex.GetHeadlessServiceName()
	assert.True(t, strings.HasSuffix(n, "-headless"))
}

func TestGetPodSpec(t *testing.T) {
	req := GetVertexPodSpecReq{
		ISBSvcType: ISBSvcTypeRedis,
		Image:      testFlowImage,
		PullPolicy: corev1.PullIfNotPresent,
		Env: []corev1.EnvVar{
			{Name: "test-env", Value: "test-val"},
		},
		SideInputsStoreName: "test-store",
		DefaultResources: corev1.ResourceRequirements{
			Requests: corev1.ResourceList{
				corev1.ResourceCPU:    resource.MustParse("100m"),
				corev1.ResourceMemory: resource.MustParse("100Mi"),
			},
			Limits: corev1.ResourceList{},
		},
	}

	t.Run("test source", func(t *testing.T) {
		testObj := testVertex.DeepCopy()
		testObj.Spec.Source = &Source{}
		testObj.Spec.AbstractPodTemplate = AbstractPodTemplate{
			NodeSelector:                 map[string]string{"a": "b"},
			Tolerations:                  []corev1.Toleration{{Key: "key", Value: "val", Operator: corev1.TolerationOpEqual}},
			SecurityContext:              &corev1.PodSecurityContext{},
			ImagePullSecrets:             []corev1.LocalObjectReference{{Name: "name"}},
			PriorityClassName:            "pname",
			Priority:                     ptr.To[int32](111),
			ServiceAccountName:           "sa",
			RuntimeClassName:             ptr.To("run"),
			AutomountServiceAccountToken: ptr.To(true),
			DNSPolicy:                    corev1.DNSClusterFirstWithHostNet,
			DNSConfig:                    &corev1.PodDNSConfig{Nameservers: []string{"aaa.aaa"}},
		}
		testObj.Spec.ContainerTemplate = &ContainerTemplate{
			Resources: corev1.ResourceRequirements{
				Requests: corev1.ResourceList{
					corev1.ResourceCPU:    resource.MustParse("200m"),
					corev1.ResourceMemory: resource.MustParse("200Mi"),
				},
				Limits: corev1.ResourceList{
					corev1.ResourceCPU:    resource.MustParse("200m"),
					corev1.ResourceMemory: resource.MustParse("200Mi"),
				},
			},
		}
		s, err := testObj.GetPodSpec(req)
		assert.NoError(t, err)
		assert.NotNil(t, s.NodeSelector)
		assert.Contains(t, s.NodeSelector, "a")
		assert.NotNil(t, s.Tolerations)
		assert.Equal(t, 1, len(s.Tolerations))
		assert.NotNil(t, s.SecurityContext)
		assert.Equal(t, 1, len(s.ImagePullSecrets))
		assert.Equal(t, "pname", s.PriorityClassName)
		assert.NotNil(t, s.Priority)
		assert.Equal(t, int32(111), *s.Priority)
		assert.Equal(t, "sa", s.ServiceAccountName)
		assert.NotNil(t, s.RuntimeClassName)
		assert.Equal(t, "run", *s.RuntimeClassName)
		assert.NotNil(t, s.AutomountServiceAccountToken)
		assert.True(t, *s.AutomountServiceAccountToken)
		assert.Equal(t, corev1.DNSClusterFirstWithHostNet, s.DNSPolicy)
		assert.Equal(t, s.DNSConfig, testObj.Spec.DNSConfig)
		assert.Equal(t, 1, len(s.Containers))
		assert.Equal(t, CtrMain, s.Containers[0].Name)
		assert.Equal(t, testFlowImage, s.Containers[0].Image)
		assert.Equal(t, corev1.PullIfNotPresent, s.Containers[0].ImagePullPolicy)
		var envNames []string
		for _, e := range s.Containers[0].Env {
			envNames = append(envNames, e.Name)
		}
		assert.Contains(t, envNames, "test-env")
		assert.Contains(t, envNames, EnvNamespace)
		assert.Contains(t, envNames, EnvPod)
		assert.Contains(t, envNames, EnvPipelineName)
		assert.Contains(t, envNames, EnvVertexName)
		assert.Contains(t, envNames, EnvVertexObject)
		assert.Contains(t, envNames, EnvReplica)
		assert.Contains(t, s.Containers[0].Args, "processor")
		assert.Contains(t, s.Containers[0].Args, "--type="+string(VertexTypeSource))
		assert.Equal(t, 2, len(s.InitContainers))
		assert.Equal(t, 2, len(s.Volumes))
		assert.Equal(t, 2, len(s.Containers[0].VolumeMounts))
		assert.Equal(t, CtrInit, s.InitContainers[0].Name)
		assert.Equal(t, CtrMonitor, s.InitContainers[1].Name)
		assert.Equal(t, "200m", s.Containers[0].Resources.Requests.Cpu().String())
		assert.Equal(t, "200m", s.Containers[0].Resources.Limits.Cpu().String())
		assert.Equal(t, "200Mi", s.Containers[0].Resources.Requests.Memory().String())
		assert.Equal(t, "200Mi", s.Containers[0].Resources.Limits.Memory().String())
		assert.Equal(t, "100m", s.InitContainers[0].Resources.Requests.Cpu().String())
		assert.Equal(t, "100Mi", s.InitContainers[0].Resources.Requests.Memory().String())
		assert.Equal(t, "0", s.InitContainers[0].Resources.Limits.Cpu().String())
		assert.Equal(t, "0", s.InitContainers[0].Resources.Limits.Memory().String())
	})

	t.Run("test sink", func(t *testing.T) {
		testObj := testVertex.DeepCopy()
		testObj.Spec.Sink = &Sink{}
		testObj.Spec.ContainerTemplate = &ContainerTemplate{
			ReadinessProbe: &Probe{
				InitialDelaySeconds: ptr.To[int32](24),
				PeriodSeconds:       ptr.To[int32](25),
				FailureThreshold:    ptr.To[int32](2),
				TimeoutSeconds:      ptr.To[int32](21),
			},
			LivenessProbe: &Probe{
				InitialDelaySeconds: ptr.To[int32](14),
				PeriodSeconds:       ptr.To[int32](15),
				FailureThreshold:    ptr.To[int32](2),
				TimeoutSeconds:      ptr.To[int32](11),
			},
		}
		s, err := testObj.GetPodSpec(req)
		assert.NoError(t, err)
		assert.Equal(t, 1, len(s.Containers))
		assert.Equal(t, CtrMain, s.Containers[0].Name)
		assert.Equal(t, 2, len(s.Volumes))
		assert.Equal(t, 2, len(s.Containers[0].VolumeMounts))
		assert.Equal(t, testFlowImage, s.Containers[0].Image)
		assert.Equal(t, corev1.PullIfNotPresent, s.Containers[0].ImagePullPolicy)
		assert.NotNil(t, s.Containers[0].ReadinessProbe)
		assert.NotNil(t, s.Containers[0].ReadinessProbe.HTTPGet)
		assert.Equal(t, "/readyz", s.Containers[0].ReadinessProbe.HTTPGet.Path)
		assert.Equal(t, int32(24), s.Containers[0].ReadinessProbe.InitialDelaySeconds)
		assert.Equal(t, int32(25), s.Containers[0].ReadinessProbe.PeriodSeconds)
		assert.Equal(t, int32(2), s.Containers[0].ReadinessProbe.FailureThreshold)
		assert.Equal(t, int32(21), s.Containers[0].ReadinessProbe.TimeoutSeconds)
		assert.Equal(t, corev1.URISchemeHTTPS, s.Containers[0].ReadinessProbe.HTTPGet.Scheme)
		assert.Equal(t, VertexMetricsPort, s.Containers[0].ReadinessProbe.HTTPGet.Port.IntValue())
		assert.NotNil(t, s.Containers[0].LivenessProbe)
		assert.NotNil(t, s.Containers[0].LivenessProbe.HTTPGet)
		assert.Equal(t, "/livez", s.Containers[0].LivenessProbe.HTTPGet.Path)
		assert.Equal(t, int32(14), s.Containers[0].LivenessProbe.InitialDelaySeconds)
		assert.Equal(t, int32(15), s.Containers[0].LivenessProbe.PeriodSeconds)
		assert.Equal(t, int32(2), s.Containers[0].LivenessProbe.FailureThreshold)
		assert.Equal(t, int32(11), s.Containers[0].LivenessProbe.TimeoutSeconds)
		assert.Equal(t, corev1.URISchemeHTTPS, s.Containers[0].LivenessProbe.HTTPGet.Scheme)
		assert.Equal(t, VertexMetricsPort, s.Containers[0].LivenessProbe.HTTPGet.Port.IntValue())
		assert.Equal(t, 1, len(s.Containers[0].Ports))
		assert.Equal(t, VertexMetricsPort, int(s.Containers[0].Ports[0].ContainerPort))
		var envNames []string
		for _, e := range s.Containers[0].Env {
			envNames = append(envNames, e.Name)
		}
		assert.Contains(t, envNames, "test-env")
		assert.Contains(t, envNames, EnvNamespace)
		assert.Contains(t, envNames, EnvPod)
		assert.Contains(t, envNames, EnvPipelineName)
		assert.Contains(t, envNames, EnvVertexName)
		assert.Contains(t, envNames, EnvVertexObject)
		assert.Contains(t, envNames, EnvReplica)
		assert.Contains(t, s.Containers[0].Args, "processor")
		assert.Contains(t, s.Containers[0].Args, "--type="+string(VertexTypeSink))
		assert.Equal(t, 2, len(s.InitContainers))
		assert.Equal(t, CtrInit, s.InitContainers[0].Name)
		assert.Equal(t, CtrMonitor, s.InitContainers[1].Name)
	})

	t.Run("test user-defined sink", func(t *testing.T) {
		testObj := testVertex.DeepCopy()
		testObj.Spec.Sink = &Sink{
			AbstractSink: AbstractSink{
				UDSink: &UDSink{
					Container: &Container{
						Image:   "image",
						Command: []string{"cmd"},
						Args:    []string{"arg0"},
					},
				},
			},
		}
		s, err := testObj.GetPodSpec(req)
		assert.NoError(t, err)
		assert.Equal(t, 1, len(s.Containers))
		assert.Equal(t, 3, len(s.InitContainers))
		assert.Equal(t, "image", s.InitContainers[2].Image)
		assert.Equal(t, 1, len(s.InitContainers[2].Command))
		assert.Equal(t, "cmd", s.InitContainers[2].Command[0])
		assert.Equal(t, 1, len(s.InitContainers[2].Args))
		assert.Equal(t, "arg0", s.InitContainers[2].Args[0])
		var sidecarEnvNames []string
		for _, env := range s.InitContainers[2].Env {
			sidecarEnvNames = append(sidecarEnvNames, env.Name)
		}
		assert.Contains(t, sidecarEnvNames, EnvCPULimit)
		assert.Contains(t, sidecarEnvNames, EnvMemoryLimit)
		assert.Contains(t, sidecarEnvNames, EnvCPURequest)
		assert.Contains(t, sidecarEnvNames, EnvMemoryRequest)
		assert.Equal(t, 2, len(s.Volumes))
		assert.Equal(t, 2, len(s.Containers[0].VolumeMounts))
	})

	t.Run("test user-defined source, with a source transformer", func(t *testing.T) {
		testObj := testVertex.DeepCopy()
		testObj.Spec.Source = &Source{
			UDSource: &UDSource{
				Container: &Container{
					Image:   "image",
					Command: []string{"cmd"},
					Args:    []string{"arg0"},
				},
			},
			UDTransformer: &UDTransformer{
				Container: &Container{
					Image:   "image",
					Command: []string{"cmd"},
					Args:    []string{"arg0"},
				},
			},
		}
		s, err := testObj.GetPodSpec(req)
		assert.NoError(t, err)
		assert.Equal(t, 1, len(s.Containers))
		assert.Equal(t, 4, len(s.InitContainers))
		for i := 2; i < len(s.InitContainers); i++ {
			assert.Equal(t, "image", s.InitContainers[i].Image)
			assert.Equal(t, 1, len(s.InitContainers[i].Command))
			assert.Equal(t, "cmd", s.InitContainers[i].Command[0])
			assert.Equal(t, 1, len(s.InitContainers[i].Args))
			assert.Equal(t, "arg0", s.InitContainers[i].Args[0])
			var sidecarEnvNames []string
			for _, env := range s.InitContainers[i].Env {
				sidecarEnvNames = append(sidecarEnvNames, env.Name)
			}
			assert.Contains(t, sidecarEnvNames, EnvCPULimit)
			assert.Contains(t, sidecarEnvNames, EnvMemoryLimit)
			assert.Contains(t, sidecarEnvNames, EnvCPURequest)
			assert.Contains(t, sidecarEnvNames, EnvMemoryRequest)
		}
	})

	t.Run("test udf", func(t *testing.T) {
		testObj := testVertex.DeepCopy()
		testObj.Spec.UDF = &UDF{
			Container: &Container{
				Image: "test-image",
			},
		}
		s, err := testObj.GetPodSpec(req)
		assert.NoError(t, err)
		assert.Equal(t, 1, len(s.Containers))
		assert.Equal(t, CtrMain, s.Containers[0].Name)
		assert.Equal(t, testFlowImage, s.Containers[0].Image)
		assert.Equal(t, corev1.PullIfNotPresent, s.Containers[0].ImagePullPolicy)
		var envNames []string
		for _, e := range s.Containers[0].Env {
			envNames = append(envNames, e.Name)
		}
		assert.Contains(t, envNames, "test-env")
		assert.Contains(t, envNames, EnvNamespace)
		assert.Contains(t, envNames, EnvPod)
		assert.Contains(t, envNames, EnvPipelineName)
		assert.Contains(t, envNames, EnvVertexName)
		assert.Contains(t, envNames, EnvVertexObject)
		assert.Contains(t, envNames, EnvReplica)
		assert.Contains(t, s.Containers[0].Args, "processor")
		assert.Contains(t, s.Containers[0].Args, "--type="+string(VertexTypeMapUDF))
		assert.Equal(t, 3, len(s.InitContainers))
		assert.Equal(t, CtrInit, s.InitContainers[0].Name)
		assert.Equal(t, CtrMonitor, s.InitContainers[1].Name)
		assert.Equal(t, CtrUdf, s.InitContainers[2].Name)
		var sidecarEnvNames []string
		for _, env := range s.InitContainers[1].Env {
			sidecarEnvNames = append(sidecarEnvNames, env.Name)
		}
		assert.Contains(t, sidecarEnvNames, EnvCPULimit)
		assert.Contains(t, sidecarEnvNames, EnvMemoryLimit)
		assert.Contains(t, sidecarEnvNames, EnvCPURequest)
		assert.Contains(t, sidecarEnvNames, EnvMemoryRequest)

	})

	t.Run("test udf with side inputs", func(t *testing.T) {
		testObj := testVertex.DeepCopy()
		testObj.Spec.SideInputs = []string{"input1", "input2"}
		testObj.Spec.UDF = &UDF{
			Container: &Container{
				Image: "test-image",
			},
		}
		s, err := testObj.GetPodSpec(req)
		assert.NoError(t, err)
		assert.Equal(t, 2, len(s.Containers))
		assert.Equal(t, CtrMain, s.Containers[0].Name)
		assert.Equal(t, CtrSideInputsWatcher, s.Containers[1].Name)
		assert.Equal(t, 4, len(s.InitContainers))
		// init container
		assert.Equal(t, CtrInit, s.InitContainers[0].Name)
		// init side inputs container
		assert.Equal(t, CtrInitSideInputs, s.InitContainers[1].Name)
		assert.Equal(t, 1, len(s.InitContainers[1].VolumeMounts))
		assert.Equal(t, "var-run-side-inputs", s.InitContainers[1].VolumeMounts[0].Name)
		assert.False(t, s.InitContainers[1].VolumeMounts[0].ReadOnly)
		// monitor container
		assert.Equal(t, CtrMonitor, s.InitContainers[2].Name)
		assert.Equal(t, 1, len(s.InitContainers[2].VolumeMounts))
		assert.Equal(t, "runtime-vol", s.InitContainers[2].VolumeMounts[0].Name)
		// udf container
		assert.Equal(t, CtrUdf, s.InitContainers[3].Name)
		assert.Equal(t, 3, len(s.InitContainers[3].VolumeMounts))
		assert.Equal(t, "var-run-side-inputs", s.InitContainers[3].VolumeMounts[2].Name)
		assert.True(t, s.InitContainers[3].VolumeMounts[2].ReadOnly)

		assert.Equal(t, 1, len(s.Containers[1].VolumeMounts))
		assert.Equal(t, "var-run-side-inputs", s.Containers[1].VolumeMounts[0].Name)
		assert.False(t, s.Containers[1].VolumeMounts[0].ReadOnly)
	})
}

func Test_getType(t *testing.T) {
	t.Run("test get source", func(t *testing.T) {
		testObj := testVertex.DeepCopy()
		testObj.Spec.Source = &Source{}
		_, ok := testObj.Spec.getType().(*Source)
		assert.True(t, ok)
	})

	t.Run("test get sink", func(t *testing.T) {
		testObj := testVertex.DeepCopy()
		testObj.Spec.Sink = &Sink{}
		_, ok := testObj.Spec.getType().(*Sink)
		assert.True(t, ok)
	})

	t.Run("test get udf", func(t *testing.T) {
		testObj := testVertex.DeepCopy()
		testObj.Spec.UDF = &UDF{}
		_, ok := testObj.Spec.getType().(*UDF)
		assert.True(t, ok)
	})
}

func TestVertexMarkPhase(t *testing.T) {
	s := VertexStatus{}
	s.MarkPhase(VertexPhaseRunning, "reason", "message")
	assert.Equal(t, VertexPhaseRunning, s.Phase)
	assert.Equal(t, "reason", s.Reason)
	assert.Equal(t, "message", s.Message)
}

func TestVertexMarkPhaseRunning(t *testing.T) {
	s := VertexStatus{}
	s.MarkPhaseRunning()
	assert.Equal(t, VertexPhaseRunning, s.Phase)
}

func TestVertexMarkPhaseFailed(t *testing.T) {
	s := VertexStatus{}
	s.MarkPhaseFailed("reason", "message")
	assert.Equal(t, VertexPhaseFailed, s.Phase)
	assert.Equal(t, "reason", s.Reason)
	assert.Equal(t, "message", s.Message)
}

func Test_VertexMarkPodNotHealthy(t *testing.T) {
	s := VertexStatus{}
	s.MarkPodNotHealthy("reason", "message")
	for _, c := range s.Conditions {
		if c.Type == string(VertexConditionPodsHealthy) {
			assert.Equal(t, metav1.ConditionFalse, c.Status)
			assert.Equal(t, "reason", c.Reason)
			assert.Equal(t, "message", c.Message)
		}
	}
}

func Test_VertexMarkPodHealthy(t *testing.T) {
	s := VertexStatus{}
	s.MarkPodHealthy("reason", "message")
	for _, c := range s.Conditions {
		if c.Type == string(VertexConditionPodsHealthy) {
			assert.Equal(t, metav1.ConditionTrue, c.Status)
			assert.Equal(t, "reason", c.Reason)
			assert.Equal(t, "message", c.Message)
		}
	}
}

func Test_VertexMarkDeployed(t *testing.T) {
	s := VertexStatus{}
	s.MarkDeployed()
	for _, c := range s.Conditions {
		if c.Type == string(VertexConditionDeployed) {
			assert.Equal(t, metav1.ConditionTrue, c.Status)
			assert.Equal(t, "Successful", c.Reason)
			assert.Equal(t, "Successful", c.Message)
		}
	}
}

func Test_VertexMarkDeployFailed(t *testing.T) {
	s := VertexStatus{}
	s.MarkDeployFailed("reason", "message")
	assert.Equal(t, VertexPhaseFailed, s.Phase)
	assert.Equal(t, "reason", s.Reason)
	assert.Equal(t, "message", s.Message)
	for _, c := range s.Conditions {
		if c.Type == string(VertexConditionDeployed) {
			assert.Equal(t, metav1.ConditionFalse, c.Status)
			assert.Equal(t, "reason", c.Reason)
			assert.Equal(t, "message", c.Message)
		}
	}
}

func Test_VertexInitConditions(t *testing.T) {
	v := VertexStatus{}
	v.InitConditions()
	assert.Equal(t, 2, len(v.Conditions))
	for _, c := range v.Conditions {
		assert.Equal(t, metav1.ConditionUnknown, c.Status)
	}
}

func Test_VertexIsSource(t *testing.T) {
	o := testVertex.DeepCopy()
	o.Spec.Source = &Source{}
	assert.True(t, o.IsASource())
	assert.False(t, o.IsUDSource())
	o.Spec.Source.UDSource = &UDSource{}
	assert.True(t, o.IsUDSource())
}

func Test_VertexHasTransformer(t *testing.T) {
	o := testVertex.DeepCopy()
	o.Spec.Source = &Source{
		UDTransformer: &UDTransformer{},
	}
	assert.True(t, o.HasUDTransformer())
}

func Test_VertexHasFallbackUDSink(t *testing.T) {
	o := testVertex.DeepCopy()
	o.Spec.Sink = &Sink{
		AbstractSink: AbstractSink{
			Log: &Log{},
		},
		Fallback: &AbstractSink{
			Log: &Log{},
		},
	}
	assert.False(t, o.HasFallbackUDSink())
	o.Spec.Sink.Fallback = &AbstractSink{
		UDSink: &UDSink{},
	}
	assert.True(t, o.HasFallbackUDSink())
}

func Test_VertexIsSink(t *testing.T) {
	o := testVertex.DeepCopy()
	o.Spec.Sink = &Sink{}
	assert.True(t, o.IsASink())
	assert.False(t, o.IsUDSink())
	o.Spec.Sink.UDSink = &UDSink{}
	assert.True(t, o.IsUDSink())
}

func Test_VertexGetInitContainers(t *testing.T) {
	req := GetVertexPodSpecReq{
		ISBSvcType: ISBSvcTypeRedis,
		Image:      testFlowImage,
		PullPolicy: corev1.PullIfNotPresent,
		Env: []corev1.EnvVar{
			{Name: "test-env", Value: "test-val"},
		},
	}
	o := testVertex.DeepCopy()
	o.Spec.Sink = &Sink{}
	o.Spec.InitContainers = []corev1.Container{
		{Name: "my-test-init", Image: "my-test-init-image"},
	}
	o.Spec.InitContainerTemplate = &ContainerTemplate{Resources: testResources}
	s := o.getInitContainers(req)
	assert.Len(t, s, 2)
	assert.Equal(t, CtrInit, s[0].Name)
	assert.Equal(t, s[0].Resources, testResources)
	assert.Equal(t, "my-test-init", s[1].Name)
	assert.Equal(t, "my-test-init-image", s[1].Image)
	assert.Equal(t, s[1].Resources, corev1.ResourceRequirements{})
	var a []string
	for _, env := range s[0].Env {
		a = append(a, env.Name)
	}
	for _, env := range s[0].Env {
		assert.Contains(t, a, env.Name)
	}
}

func TestScalable(t *testing.T) {
	v := Vertex{}
	v.Spec.Scale.Disabled = true
	assert.False(t, v.Scalable())
	v.Spec.Scale.Disabled = false
	v.Spec.Sink = &Sink{}
	assert.True(t, v.Scalable())
	v.Spec.Sink = nil
	v.Spec.UDF = &UDF{}
	assert.True(t, v.Scalable())
	v.Spec.UDF = &UDF{
		GroupBy: &GroupBy{},
	}
	assert.False(t, v.Scalable())
	v.Spec.UDF = nil
	v.Spec.Source = &Source{
		HTTP: &HTTPSource{},
	}
	assert.True(t, v.Scalable())
	v.Spec.Source = &Source{
		Kafka: &KafkaSource{},
	}
	assert.True(t, v.Scalable())
	v.Spec.Source = &Source{
		UDSource: &UDSource{},
	}
	assert.True(t, v.Scalable())
}

func Test_GetVertexType(t *testing.T) {
	t.Run("source vertex", func(t *testing.T) {
		v := Vertex{
			Spec: VertexSpec{
				AbstractVertex: AbstractVertex{
					Source: &Source{},
				},
			},
		}
		assert.Equal(t, VertexTypeSource, v.GetVertexType())
	})

	t.Run("sink vertex", func(t *testing.T) {
		v := Vertex{
			Spec: VertexSpec{
				AbstractVertex: AbstractVertex{
					Sink: &Sink{},
				},
			},
		}
		assert.Equal(t, VertexTypeSink, v.GetVertexType())
	})

	t.Run("udf vertex", func(t *testing.T) {
		v := Vertex{
			Spec: VertexSpec{
				AbstractVertex: AbstractVertex{
					UDF: &UDF{},
				},
			},
		}
		assert.Equal(t, VertexTypeMapUDF, v.GetVertexType())
	})

	t.Run("vertex with no type", func(t *testing.T) {
		v := Vertex{
			Spec: VertexSpec{},
		}
		assert.Equal(t, VertexType(""), v.GetVertexType())
	})
}
func Test_GetToBuckets(t *testing.T) {
	t.Run("sink vertex", func(t *testing.T) {
		v := Vertex{
			ObjectMeta: metav1.ObjectMeta{
				Namespace: "test-ns",
			},
			Spec: VertexSpec{
				PipelineName: "test-pipeline",
				AbstractVertex: AbstractVertex{
					Name: "test-vertex",
					Sink: &Sink{},
				},
			},
		}
		buckets := v.GetToBuckets()
		assert.Len(t, buckets, 1)
		assert.Equal(t, "test-ns-test-pipeline-test-vertex_SINK", buckets[0])
	})

	t.Run("non-sink vertex with edges", func(t *testing.T) {
		v := Vertex{
			ObjectMeta: metav1.ObjectMeta{
				Namespace: "test-ns",
			},
			Spec: VertexSpec{
				PipelineName: "test-pipeline",
				AbstractVertex: AbstractVertex{
					Name: "test-vertex",
				},
				ToEdges: []CombinedEdge{
					{Edge: Edge{From: "test-vertex", To: "output1"}},
					{Edge: Edge{From: "test-vertex", To: "output2"}},
				},
			},
		}
		buckets := v.GetToBuckets()
		assert.Len(t, buckets, 2)
		assert.Contains(t, buckets, "test-ns-test-pipeline-test-vertex-output1")
		assert.Contains(t, buckets, "test-ns-test-pipeline-test-vertex-output2")
	})

	t.Run("non-sink vertex without edges", func(t *testing.T) {
		v := Vertex{
			ObjectMeta: metav1.ObjectMeta{
				Namespace: "test-ns",
			},
			Spec: VertexSpec{
				PipelineName: "test-pipeline",
				AbstractVertex: AbstractVertex{
					Name: "test-vertex",
				},
			},
		}
		buckets := v.GetToBuckets()
		assert.Len(t, buckets, 0)
	})
}

func Test_VertexStatus_IsHealthy(t *testing.T) {
	tests := []struct {
		name  string
		phase VertexPhase
		ready bool
		want  bool
	}{
		{
			name:  "Failed phase",
			phase: VertexPhaseFailed,
			ready: false,
			want:  false,
		},
		{
			name:  "Running phase and ready",
			phase: VertexPhaseRunning,
			ready: true,
			want:  true,
		},
		{
			name:  "Running phase and not ready",
			phase: VertexPhaseRunning,
			ready: false,
			want:  false,
		},
		{
			name:  "Failed phase",
			phase: VertexPhaseFailed,
			ready: false,
			want:  false,
		},
		{
			name:  "Unknown phase",
			phase: VertexPhaseUnknown,
			ready: false,
			want:  false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			vertex := &VertexStatus{
				Phase: tt.phase,
			}
			if tt.ready {
				vertex.Conditions = []metav1.Condition{
					{
						Type:   string(VertexConditionPodsHealthy),
						Status: metav1.ConditionTrue,
					},
				}
			}
			got := vertex.IsHealthy()
			assert.Equal(t, tt.want, got)
		})
	}
}
