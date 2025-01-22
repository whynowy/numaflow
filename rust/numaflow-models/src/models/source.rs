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

// Code generated by Openapi Generator. DO NOT EDIT.

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Source {
    #[serde(rename = "container", skip_serializing_if = "Option::is_none")]
    pub container: Option<Box<crate::models::Container>>,
    #[serde(rename = "generator", skip_serializing_if = "Option::is_none")]
    pub generator: Option<Box<crate::models::GeneratorSource>>,
    #[serde(rename = "http", skip_serializing_if = "Option::is_none")]
    pub http: Option<Box<crate::models::HttpSource>>,
    #[serde(rename = "jetstream", skip_serializing_if = "Option::is_none")]
    pub jetstream: Option<Box<crate::models::JetStreamSource>>,
    #[serde(rename = "kafka", skip_serializing_if = "Option::is_none")]
    pub kafka: Option<Box<crate::models::KafkaSource>>,
    #[serde(rename = "nats", skip_serializing_if = "Option::is_none")]
    pub nats: Option<Box<crate::models::NatsSource>>,
    #[serde(rename = "pulsar", skip_serializing_if = "Option::is_none")]
    pub pulsar: Option<Box<crate::models::PulsarSource>>,
    #[serde(rename = "serving", skip_serializing_if = "Option::is_none")]
    pub serving: Option<Box<crate::models::ServingSource>>,
    #[serde(rename = "transformer", skip_serializing_if = "Option::is_none")]
    pub transformer: Option<Box<crate::models::UdTransformer>>,
    #[serde(rename = "udsource", skip_serializing_if = "Option::is_none")]
    pub udsource: Option<Box<crate::models::UdSource>>,
}

impl Source {
    pub fn new() -> Source {
        Source {
            container: None,
            generator: None,
            http: None,
            jetstream: None,
            kafka: None,
            nats: None,
            pulsar: None,
            serving: None,
            transformer: None,
            udsource: None,
        }
    }
}
