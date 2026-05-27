use nodalmerge_host_core::engine::{
    ClientMessageParseClassification,
    WebRtcRelayBranchClassification,
    classify_webrtc_relay_branch,
    classify_client_message_json,
    extract_client_message_type_text,
};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientDispatchCommand {
    Subscribe,
    Pack,
    MstRequest,
    MstDone,
    Request,
    BlobUpload,
    BlobRequest,
    RequestUpload,
    BlobUploaded,
    Presence,
    SetRoomKey,
    SetPolicy,
    ServerInfo,
    StartTick,
    StopTick,
    CompactRoom,
    ArchiveDescribe,
    ArchiveValidate,
    ArchiveImport,
    ArchiveExport,
    Relay,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct ClientDispatchContext {
    pub message: Value,
    pub message_type: String,
    pub command: ClientDispatchCommand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MalformedClientMessagePlan {
    pub error_message: &'static str,
    pub keep_connection_open: bool,
}

#[derive(Debug, Clone)]
pub enum ClientDispatchBuild {
    Ready(ClientDispatchContext),
    Malformed(MalformedClientMessagePlan),
}

pub fn build_client_dispatch_context(text: &str) -> ClientDispatchBuild {
    match classify_client_message_json(text) {
        ClientMessageParseClassification::Parsed(message) => {
            let message_type = extract_client_message_type_text(&message);
            ClientDispatchBuild::Ready(ClientDispatchContext {
                command: route_client_dispatch_command(&message_type),
                message_type,
                message,
            })
        }
        ClientMessageParseClassification::MalformedJson(plan) => {
            ClientDispatchBuild::Malformed(MalformedClientMessagePlan {
                error_message: plan.error_message,
                keep_connection_open: plan.keep_connection_open,
            })
        }
    }
}

pub fn route_client_dispatch_command(message_type: &str) -> ClientDispatchCommand {
    match message_type {
        "subscribe" => ClientDispatchCommand::Subscribe,
        "pack" => ClientDispatchCommand::Pack,
        "mst-request" => ClientDispatchCommand::MstRequest,
        "mst-done" => ClientDispatchCommand::MstDone,
        "request" => ClientDispatchCommand::Request,
        "blob-upload" => ClientDispatchCommand::BlobUpload,
        "blob-request" => ClientDispatchCommand::BlobRequest,
        "request-upload" => ClientDispatchCommand::RequestUpload,
        "blob-uploaded" => ClientDispatchCommand::BlobUploaded,
        "presence" => ClientDispatchCommand::Presence,
        "set-room-key" => ClientDispatchCommand::SetRoomKey,
        "set-policy" => ClientDispatchCommand::SetPolicy,
        "server-info" => ClientDispatchCommand::ServerInfo,
        "start-tick" => ClientDispatchCommand::StartTick,
        "stop-tick" => ClientDispatchCommand::StopTick,
        "compact-room" => ClientDispatchCommand::CompactRoom,
        "archive.describe" => ClientDispatchCommand::ArchiveDescribe,
        "archive.validate" => ClientDispatchCommand::ArchiveValidate,
        "archive.import" => ClientDispatchCommand::ArchiveImport,
        "archive.export" => ClientDispatchCommand::ArchiveExport,
        _ if classify_webrtc_relay_branch(message_type)
            == WebRtcRelayBranchClassification::Relay => ClientDispatchCommand::Relay,
        _ => ClientDispatchCommand::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_archive_commands_to_control_plane_variants() {
        assert_eq!(
            route_client_dispatch_command("archive.describe"),
            ClientDispatchCommand::ArchiveDescribe
        );
        assert_eq!(
            route_client_dispatch_command("archive.validate"),
            ClientDispatchCommand::ArchiveValidate
        );
        assert_eq!(
            route_client_dispatch_command("archive.import"),
            ClientDispatchCommand::ArchiveImport
        );
        assert_eq!(
            route_client_dispatch_command("archive.export"),
            ClientDispatchCommand::ArchiveExport
        );
    }

    #[test]
    fn build_dispatch_context_preserves_archive_message_type() {
        let raw = r#"{"type":"archive.validate","archive_ref":"s3://bucket/a.nmar","mode":"metadata_only"}"#;
        let build = build_client_dispatch_context(raw);
        let ClientDispatchBuild::Ready(ctx) = build else {
            panic!("expected parsed dispatch context")
        };

        assert_eq!(ctx.message_type, "archive.validate");
        assert_eq!(ctx.command, ClientDispatchCommand::ArchiveValidate);
        assert_eq!(
            ctx.message["archive_ref"].as_str(),
            Some("s3://bucket/a.nmar")
        );
    }
}