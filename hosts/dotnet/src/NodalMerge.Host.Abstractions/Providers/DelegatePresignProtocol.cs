namespace NodalMerge.Host.Abstractions.Providers;

/// <summary>
/// Frozen constants of the delegate presign protocol v1
/// (docs/BLOB_STORAGE_LAYOUT.md §7), slice 7.4 of
/// nodalmerge-studio/plans/blob-cas-remediation.md.
///
/// The protocol's <c>room</c>/<c>namespace</c> fields are metadata only —
/// they MUST NOT influence the delegate's key derivation
/// (<c>&lt;app-prefix&gt;blake3/&lt;hash&gt;</c>) — but a delegate may log,
/// quota or authorize against them, so both runtimes must send the SAME
/// placeholder on the room-agnostic blob HTTP routes
/// (<c>GET /blobs/{hash}/url</c>, <c>POST /blobs/{hash}/uploaded</c>).
/// Before 7.4 they drifted: Rust sent <c>"_global"</c>, .NET sent
/// <c>"default"</c>/<c>"blobs"</c>.
///
/// The values are pinned cross-runtime by the
/// <c>delegate_room_id_placeholder</c> slot of
/// <c>engine/commands/work-unit-status-vectors.v1.json</c> (reserved for
/// exactly this by slice 0.4) — change them there or here and a vector test
/// fails on the other side.
///
/// The legacy <c>/sync/blob-url</c> route is NOT covered by these constants:
/// it accepts caller-supplied <c>room</c>/<c>namespace</c> query parameters
/// with its own frozen backward-compat defaults (<c>"default"</c>/
/// <c>"assets"</c> — see docs/BLOB_HTTP_SURFACE.md's legacy-route note and
/// LegacySyncBlobUrlCompatTests).
/// </summary>
public static class DelegatePresignProtocol
{
    /// <summary>
    /// The <c>room</c> placeholder every host sends on room-agnostic routes.
    /// Mirrors the Rust host's <c>blob_http::GLOBAL_ROOM_PLACEHOLDER</c> and
    /// <c>room.rs</c>'s GC-preflight placeholder: "this isn't really any one
    /// room — the CAS is global."
    /// </summary>
    public const string GlobalRoomPlaceholder = "_global";

    /// <summary>
    /// The value of the protocol's OPTIONAL <c>namespace</c> field on
    /// room-agnostic routes, for hosts that send the field at all (.NET
    /// does; the Rust delegate client omits it entirely, which remains
    /// conformant).
    /// </summary>
    public const string GlobalRoomNamespace = "blobs";
}
