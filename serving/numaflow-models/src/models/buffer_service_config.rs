/*
 * Numaflow
 *
 * No description provided (generated by Openapi Generator https://github.com/openapitools/openapi-generator)
 *
 * The version of the OpenAPI document: latest
 *
 * Generated by: https://openapi-generator.tech
 */

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BufferServiceConfig {
    #[serde(rename = "jetstream", skip_serializing_if = "Option::is_none")]
    pub jetstream: Option<Box<crate::models::JetStreamConfig>>,
    #[serde(rename = "redis", skip_serializing_if = "Option::is_none")]
    pub redis: Option<Box<crate::models::RedisConfig>>,
}

impl BufferServiceConfig {
    pub fn new() -> BufferServiceConfig {
        BufferServiceConfig {
            jetstream: None,
            redis: None,
        }
    }
}
