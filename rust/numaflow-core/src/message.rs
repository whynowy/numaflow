use std::cmp::{Ordering, PartialEq};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use crate::Error;
use crate::config::get_vertex_name;
use crate::shared::grpc::prost_timestamp_from_utc;
use bytes::{Bytes, BytesMut};
use chrono::{DateTime, Utc};
use prost::Message as ProtoMessage;
use serde::{Deserialize, Serialize};

const DROP: &str = "U+005C__DROP__";

/// The message that is passed from the source to the sink.
/// NOTE: It is cheap to clone.
#[derive(Debug, Clone)]
pub(crate) struct Message {
    /// Type of the message that flows through the ISB.
    pub(crate) typ: MessageType,
    /// keys of the message
    pub(crate) keys: Arc<[String]>,
    /// tags of the message
    pub(crate) tags: Option<Arc<[String]>>,
    /// actual payload of the message
    pub(crate) value: Bytes,
    /// offset of the message, it is optional because offset is only
    /// available when we read the message, and we don't persist the
    /// offset in the ISB.
    pub(crate) offset: Offset,
    /// event time of the message
    pub(crate) event_time: DateTime<Utc>,
    /// watermark of the message
    pub(crate) watermark: Option<DateTime<Utc>>,
    /// id of the message
    pub(crate) id: MessageID,
    /// headers of the message
    pub(crate) headers: HashMap<String, String>,
    /// Additional metadata that could be passed per message between the vertices.
    pub(crate) metadata: Option<Metadata>,
    /// is_late is used to indicate if the message is a late data. Late data is data that arrives
    /// after the watermark has passed. This is set only at source.
    pub(crate) is_late: bool,
}

/// Type of the [Message].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum MessageType {
    /// the payload is Data
    #[default]
    Data,
    /// the payload is a control message.
    #[allow(clippy::upper_case_acronyms)]
    WMB,
}

impl fmt::Display for MessageType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MessageType::Data => write!(f, "Data"),
            MessageType::WMB => write!(f, "WMB"),
        }
    }
}

// proto enum is an i32 type and WMB is defined as enum in the proto.
impl From<i32> for MessageType {
    fn from(kind: i32) -> Self {
        match kind {
            0 => MessageType::Data,
            _ => MessageType::WMB,
        }
    }
}

impl From<MessageType> for i32 {
    fn from(kind: MessageType) -> Self {
        match kind {
            MessageType::Data => 0,
            MessageType::WMB => 1,
        }
    }
}

impl Default for Message {
    fn default() -> Self {
        Self {
            keys: Arc::new([]),
            tags: None,
            value: Bytes::new(),
            offset: Default::default(),
            event_time: Utc::now(),
            watermark: None,
            id: Default::default(),
            headers: HashMap::new(),
            metadata: None,
            typ: Default::default(),
            is_late: false,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Metadata {
    /// name of the previous vertex.
    pub(crate) previous_vertex: String,
    pub(crate) sys_metadata: HashMap<String, KeyValueGroup>,
    pub(crate) user_metadata: HashMap<String, KeyValueGroup>,
}

impl Default for Metadata {
    fn default() -> Self {
        Self {
            previous_vertex: "".to_string(),
            sys_metadata: HashMap::new(),
            user_metadata: HashMap::new(),
        }
    }
}

impl From<numaflow_pb::objects::isb::Metadata> for Metadata {
    fn from(metadata: numaflow_pb::objects::isb::Metadata) -> Self {
        Self {
            previous_vertex: metadata.previous_vertex,
            sys_metadata: metadata
                .sys_metadata
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
            user_metadata: metadata
                .user_metadata
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
        }
    }
}

impl From<Metadata> for numaflow_pb::objects::isb::Metadata {
    fn from(metadata: Metadata) -> Self {
        Self {
            // write the current vertex name so that next vertex knows who sent it.
            previous_vertex: get_vertex_name().to_string(),
            sys_metadata: metadata
                .sys_metadata
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
            user_metadata: metadata
                .user_metadata
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct KeyValueGroup {
    pub(crate) group: HashMap<String, KeyValue>,
}

impl From<numaflow_pb::objects::isb::KeyValueGroup> for KeyValueGroup {
    fn from(group: numaflow_pb::objects::isb::KeyValueGroup) -> Self {
        Self {
            group: group
                .key_value
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
        }
    }
}

impl From<KeyValueGroup> for numaflow_pb::objects::isb::KeyValueGroup {
    fn from(group: KeyValueGroup) -> Self {
        Self {
            key_value: group
                .group
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct KeyValue {
    pub(crate) key: String,
    pub(crate) value: Bytes,
}

impl From<numaflow_pb::objects::isb::KeyValue> for KeyValue {
    fn from(kv: numaflow_pb::objects::isb::KeyValue) -> Self {
        Self {
            key: kv.key,
            value: kv.value.into(),
        }
    }
}

impl From<KeyValue> for numaflow_pb::objects::isb::KeyValue {
    fn from(kv: KeyValue) -> Self {
        Self {
            key: kv.key,
            value: kv.value.to_vec(),
        }
    }
}

impl TryFrom<KeyValue> for BytesMut {
    type Error = Error;

    fn try_from(kv: KeyValue) -> Result<Self, Self::Error> {
        let proto_kv = numaflow_pb::objects::isb::KeyValue {
            key: kv.key,
            value: kv.value.to_vec(),
        };

        let mut buf = BytesMut::new();
        proto_kv
            .encode(&mut buf)
            .map_err(|e| Error::Proto(e.to_string()))?;
        Ok(buf)
    }
}

/// Offset of the message which will be used to acknowledge the message.
#[derive(Debug, Clone, Hash, Serialize, Deserialize, Eq, PartialEq)]
pub(crate) enum Offset {
    Int(IntOffset),
    String(StringOffset),
}

impl Offset {
    pub(crate) fn partition_idx(&self) -> u16 {
        match self {
            Offset::Int(offset) => offset.partition_idx,
            Offset::String(offset) => offset.partition_idx,
        }
    }
}

impl fmt::Display for Offset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Offset::Int(offset) => write!(f, "{offset}"),
            Offset::String(offset) => write!(f, "{offset}"),
        }
    }
}

impl PartialOrd for Offset {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Offset {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Offset::Int(a), Offset::Int(b)) => a.cmp(b),
            (Offset::String(a), Offset::String(b)) => a.cmp(b),
            _ => Ordering::Equal,
        }
    }
}

impl Default for Offset {
    fn default() -> Self {
        Offset::Int(Default::default())
    }
}

impl Message {
    // Check if the message should be dropped.
    pub(crate) fn dropped(&self) -> bool {
        self.tags
            .as_ref()
            .is_some_and(|tags| tags.contains(&DROP.to_string()))
    }
}

/// IntOffset is integer based offset enum type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub(crate) struct IntOffset {
    pub(crate) offset: i64,
    pub(crate) partition_idx: u16,
}

impl IntOffset {
    pub(crate) fn new(seq: i64, partition_idx: u16) -> Self {
        Self {
            offset: seq,
            partition_idx,
        }
    }
}

impl PartialOrd for IntOffset {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for IntOffset {
    fn cmp(&self, other: &Self) -> Ordering {
        self.offset.cmp(&other.offset)
    }
}

impl fmt::Display for IntOffset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.offset, self.partition_idx)
    }
}

/// StringOffset is string based offset enum type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub(crate) struct StringOffset {
    /// offset could be a complex base64 string.
    pub(crate) offset: Bytes,
    pub(crate) partition_idx: u16,
}

impl StringOffset {
    pub fn new(seq: String, partition_idx: u16) -> Self {
        Self {
            offset: seq.into(),
            partition_idx,
        }
    }
}

impl PartialOrd for StringOffset {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for StringOffset {
    fn cmp(&self, other: &Self) -> Ordering {
        self.offset.cmp(&other.offset)
    }
}

impl fmt::Display for StringOffset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}-{}",
            std::str::from_utf8(&self.offset).expect("it should be valid utf-8"),
            self.partition_idx
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) enum ReadAck {
    /// Message was successfully processed.
    Ack,
    /// Message will not be processed now and processing can move onto the next message, NAK’d message will be retried.
    Nak,
}

/// Message ID which is used to uniquely identify a message. It cheap to clone this.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct MessageID {
    pub(crate) vertex_name: Bytes,
    pub(crate) offset: Bytes,
    /// Index is used to identify the index of the message in case of flatmap.
    pub(crate) index: i32,
}

impl Default for MessageID {
    fn default() -> Self {
        Self {
            vertex_name: Bytes::new(),
            offset: Bytes::new(),
            index: 0,
        }
    }
}

impl From<numaflow_pb::objects::isb::MessageId> for MessageID {
    fn from(id: numaflow_pb::objects::isb::MessageId) -> Self {
        Self {
            vertex_name: id.vertex_name.into(),
            offset: id.offset.into(),
            index: id.index,
        }
    }
}

impl From<MessageID> for numaflow_pb::objects::isb::MessageId {
    fn from(id: MessageID) -> Self {
        Self {
            vertex_name: String::from_utf8_lossy(&id.vertex_name).to_string(),
            offset: String::from_utf8_lossy(&id.offset).to_string(),
            index: id.index,
        }
    }
}
impl fmt::Display for MessageID {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}-{}-{}",
            std::str::from_utf8(&self.vertex_name).expect("it should be valid utf-8"),
            std::str::from_utf8(&self.offset).expect("it should be valid utf-8"),
            self.index
        )
    }
}

impl TryFrom<Message> for Bytes {
    type Error = Error;

    fn try_from(value: Message) -> Result<Self, Self::Error> {
        let b: BytesMut = value.try_into()?;
        Ok(b.freeze())
    }
}

impl TryFrom<Message> for BytesMut {
    type Error = Error;

    fn try_from(message: Message) -> Result<Self, Self::Error> {
        let proto_message = numaflow_pb::objects::isb::Message {
            header: Some(numaflow_pb::objects::isb::Header {
                message_info: Some(numaflow_pb::objects::isb::MessageInfo {
                    event_time: Some(prost_timestamp_from_utc(message.event_time)),
                    is_late: message.is_late,
                }),
                kind: message.typ.into(),
                id: Some(message.id.into()),
                keys: message.keys.to_vec(),
                headers: message.headers,
                metadata: message.metadata.map(|m| m.into()),
            }),
            body: Some(numaflow_pb::objects::isb::Body {
                payload: message.value.to_vec(),
            }),
        };

        let mut buf = BytesMut::new();
        proto_message
            .encode(&mut buf)
            .map_err(|e| Error::Proto(e.to_string()))?;
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::TimeZone;
    use numaflow_pb::objects::isb::{
        Body, Header, Message as ProtoMessage, MessageId, MessageInfo,
    };

    use super::*;
    use crate::error::Result;

    #[test]
    fn test_offset_display() {
        let offset = Offset::String(StringOffset {
            offset: "123".to_string().into(),
            partition_idx: 1,
        });
        assert_eq!(format!("{}", offset), "123-1");
    }

    #[test]
    fn test_message_id_display() {
        let message_id = MessageID {
            vertex_name: "vertex".to_string().into(),
            offset: "123".to_string().into(),
            index: 0,
        };
        assert_eq!(format!("{}", message_id), "vertex-123-0");
    }

    #[test]
    fn test_message_to_vec_u8() {
        let message = Message {
            typ: Default::default(),
            keys: Arc::from(vec!["key1".to_string()]),
            tags: None,
            value: vec![1, 2, 3].into(),
            offset: Offset::String(StringOffset {
                offset: "123".to_string().into(),
                partition_idx: 0,
            }),
            event_time: Utc.timestamp_opt(1627846261, 0).unwrap(),
            watermark: None,
            id: MessageID {
                vertex_name: "vertex".to_string().into(),
                offset: "123".to_string().into(),
                index: 0,
            },
            headers: HashMap::new(),
            metadata: None,
            is_late: false,
        };

        let result: Result<BytesMut> = message.clone().try_into();
        assert!(result.is_ok());

        let proto_message = ProtoMessage {
            header: Some(Header {
                message_info: Some(MessageInfo {
                    event_time: Some(prost_timestamp_from_utc(message.event_time)),
                    is_late: false,
                }),
                kind: numaflow_pb::objects::isb::MessageKind::Data as i32,
                id: Some(message.id.into()),
                keys: message.keys.to_vec(),
                headers: message.headers,
                metadata: message.metadata.map(|m| m.into()),
            }),
            body: Some(Body {
                payload: message.value.clone().into(),
            }),
        };

        let mut buf = Vec::new();
        prost::Message::encode(&proto_message, &mut buf).unwrap();
        assert_eq!(result.unwrap(), buf);
    }

    #[test]
    fn test_message_id_from_proto() {
        let proto_id = MessageId {
            vertex_name: "vertex".to_string(),
            offset: "123".to_string(),
            index: 0,
        };
        let message_id: MessageID = proto_id.into();
        assert_eq!(message_id.vertex_name, "vertex");
        assert_eq!(message_id.offset, "123");
        assert_eq!(message_id.index, 0);
    }

    #[test]
    fn test_message_id_to_proto() {
        let message_id = MessageID {
            vertex_name: "vertex".to_string().into(),
            offset: "123".to_string().into(),
            index: 0,
        };
        let proto_id: MessageId = message_id.into();
        assert_eq!(proto_id.vertex_name, "vertex");
        assert_eq!(proto_id.offset, "123");
        assert_eq!(proto_id.index, 0);
    }

    #[test]
    fn test_offset_cases() {
        let int_offset = IntOffset::new(42, 1);
        assert_eq!(int_offset.offset, 42);
        assert_eq!(int_offset.partition_idx, 1);
        assert_eq!(format!("{}", int_offset), "42-1");

        let string_offset = StringOffset::new("42".to_string(), 1);
        assert_eq!(string_offset.offset, "42");
        assert_eq!(string_offset.partition_idx, 1);
        assert_eq!(format!("{}", string_offset), "42-1");

        let offset_int = Offset::Int(int_offset);
        assert_eq!(format!("{}", offset_int), "42-1");

        let offset_string = Offset::String(string_offset);
        assert_eq!(format!("{}", offset_string), "42-1");
    }
}
