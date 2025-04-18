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
pub struct ServingSpec {
    #[serde(rename = "auth", skip_serializing_if = "Option::is_none")]
    pub auth: Option<Box<crate::models::Authorization>>,
    #[serde(rename = "containerTemplate", skip_serializing_if = "Option::is_none")]
    pub container_template: Option<Box<crate::models::ContainerTemplate>>,
    /// The header key from which the message id will be extracted
    #[serde(rename = "msgIDHeaderKey")]
    pub msg_id_header_key: String,
    /// Number of replicas. If an HPA is used to manage the deployment object, do not set this field.
    #[serde(rename = "replicas", skip_serializing_if = "Option::is_none")]
    pub replicas: Option<i32>,
    /// Request timeout in seconds. Default value is 120 seconds.
    #[serde(
        rename = "requestTimeoutSeconds",
        skip_serializing_if = "Option::is_none"
    )]
    pub request_timeout_seconds: Option<i64>,
    /// Whether to create a ClusterIP Service
    #[serde(rename = "service", skip_serializing_if = "Option::is_none")]
    pub service: Option<bool>,
    #[serde(rename = "store", skip_serializing_if = "Option::is_none")]
    pub store: Option<Box<crate::models::ServingStore>>,
}

impl ServingSpec {
    pub fn new(msg_id_header_key: String) -> ServingSpec {
        ServingSpec {
            auth: None,
            container_template: None,
            msg_id_header_key,
            replicas: None,
            request_timeout_seconds: None,
            service: None,
            store: None,
        }
    }
}
