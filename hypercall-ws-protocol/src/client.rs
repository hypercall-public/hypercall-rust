use serde::{Deserialize, Serialize};

/// Client-originated control messages accepted on `/ws`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientControlMessage {
    /// Subscribe to a data channel.
    Subscribe {
        channel: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        symbols: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expiry: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        option_type: Option<String>,
    },
    /// Unsubscribe from a data channel.
    Unsubscribe {
        channel: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        symbols: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expiry: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        option_type: Option<String>,
    },
    /// Identify the connection with a wallet address.
    Authenticate { wallet: String },
}

/// Gateway-originated backend status sent to clients while preserving sockets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayStatus {
    BackendReconnecting,
    BackendReady,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum GatewayStatusMessage {
    GatewayStatus {
        status: GatewayStatus,
    },
    GatewayError {
        code: GatewayErrorCode,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayErrorCode {
    UnsupportedWrite,
    UpstreamUnavailable,
    ProtocolError,
}

#[cfg(test)]
mod tests {
    use super::{ClientControlMessage, GatewayErrorCode, GatewayStatus, GatewayStatusMessage};

    #[test]
    fn subscribe_preserves_existing_wire_shape() {
        let json = r#"{"type":"Subscribe","channel":"orderbook","symbols":["BTC"]}"#;
        let msg: ClientControlMessage = serde_json::from_str(json).unwrap();

        assert_eq!(
            msg,
            ClientControlMessage::Subscribe {
                channel: "orderbook".to_string(),
                symbols: Some(vec!["BTC".to_string()]),
                expiry: None,
                option_type: None,
            }
        );
        assert_eq!(serde_json::to_string(&msg).unwrap(), json);
    }

    #[test]
    fn gateway_status_uses_snake_case_status_values() {
        let msg = GatewayStatusMessage::GatewayStatus {
            status: GatewayStatus::BackendReconnecting,
        };

        assert_eq!(
            serde_json::to_string(&msg).unwrap(),
            r#"{"type":"GatewayStatus","status":"backend_reconnecting"}"#
        );
    }

    #[test]
    fn gateway_error_wire_shape_is_stable() {
        let msg = GatewayStatusMessage::GatewayError {
            code: GatewayErrorCode::UnsupportedWrite,
            message: "write commands are not supported on websocket".to_string(),
        };

        assert_eq!(
            serde_json::to_string(&msg).unwrap(),
            r#"{"type":"GatewayError","code":"unsupported_write","message":"write commands are not supported on websocket"}"#
        );
    }
}
