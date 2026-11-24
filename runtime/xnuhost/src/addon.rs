// SPDX-License-Identifier: MIT
//! The boot addon a runtime package carries: `kernel/gpuaddon`, the `NGPUADD2` sidecar `oneios-tool ios addon` writes
//! (the Metal plug-in under `addons/`, the capture plug-in under `camera/`), and the CDHashes `kernel/gpuaddon.cdhashes`
//! lists. The kernel writes its files into the Data store and mounts them over the sealed system's folders before
//! launchd starts (hosted 0031, 0056, 0090).
//!
//! The image set's own sidecar (`<kernel>.gpuaddon`, staged by `oneios-tool phone addon`) is the one a phone chose, and
//! it wins (session/board.rs). The runtime's is the fallback, so a plug-in reaches the guest with the runtime alone — no
//! adb, no image rebuild — and its CDHashes join the trust cache this boot gives the kernel: an image built without the
//! plug-in has no owner module that names it (`overlay/userland/owner-trust.json`).

use xh_core::bytes::le;
use xh_core::{loge, logi};

/// A boot addon and the CDHashes its code needs in this boot's trust cache.
pub struct Addon {
    pub blob: Vec<u8>,
    /// The 20-byte CDHashes of the files the addon carries (the first word of each line of the list `ios addon` writes).
    pub cdhashes: Vec<[u8; 20]>,
}

/// The runtime's boot addon: `kernel/gpuaddon` under the runtime's folder, with `kernel/gpuaddon.cdhashes`. None when
/// the runtime has none, or when the file there is not a boot addon for this image (left alone, said in the log).
pub fn runtime_addon(runtime: &str, expected_target: &str) -> Option<Addon> {
    let path = format!("{}/kernel/gpuaddon", runtime.trim_end_matches('/'));
    let target_path = format!("{path}.target");
    let target = crate::session::read_file(&target_path, 256)?;
    let target_text = std::str::from_utf8(&target).ok()?;
    let target = target_text.trim();
    if target_text.lines().count() != 1 || target != expected_target {
        loge!("gpuaddon: {target_path}: targets {target:?}, this image is {expected_target:?}: left alone");
        return None;
    }
    let blob = crate::session::read_file(&path, 64 << 20)?;
    // The loader checks the same magic and total: "NGPUADD2" and a total that is the file's length (ios addon writes
    // 24 + the body). A cut or padded file is refused here, where the log can name the runtime.
    if blob.len() < 24 || blob.get(..8) != Some(b"NGPUADD2".as_slice()) || le::u64(&blob, 16) != blob.len() as u64 {
        loge!("gpuaddon: {path}: not a boot addon (NGPUADD2, its total is not the file's length): left alone");
        return None;
    }
    if !contains_metal(&blob) {
        loge!("gpuaddon: {path}: addon has no complete NativeAdrenoMetal bundle: left alone");
        return None;
    }
    // A bundle with no CDHash list would mount but fail code-signing trust at process launch, looking like a silent
    // Metal failure. Do not hand such a payload to the kernel.
    let hashes_path = format!("{path}.cdhashes");
    let Some(list) = crate::session::read_file(&hashes_path, 1 << 16) else {
        loge!("gpuaddon: {hashes_path} is missing: left alone because its code would not be trusted");
        return None;
    };
    let cdhashes = match parse_cdhashes(&list) {
        Ok(cdhashes) if !cdhashes.is_empty() => cdhashes,
        Ok(_) => {
            loge!("gpuaddon: {hashes_path} lists no CDHashes: left alone because its code would not be trusted");
            return None;
        }
        Err(why) => {
            loge!("gpuaddon: {hashes_path}: {why}: left alone");
            return None;
        }
    };
    let text = std::str::from_utf8(&list).ok()?;
    if !text.lines().any(|line| line.split_whitespace().nth(1) == Some("addons/NativeAdrenoMetal.bundle/NativeAdrenoMetal"))
        || !text.lines().any(|line| line.split_whitespace().nth(1) == Some("addons/NativeAdrenoMetal.bundle/libvulkan_freedreno.dylib"))
    {
        loge!("gpuaddon: {hashes_path}: CDHash list does not cover the Metal executable and Turnip library: left alone");
        return None;
    }
    logi!(
        "gpuaddon: {path}: {} bytes, {} CDHashes trusted (the runtime's; the image set has no kernel.kc.gpuaddon)",
        blob.len(),
        cdhashes.len()
    );
    Some(Addon { blob, cdhashes })
}

/// The addon is an NGPUADD2 archive. Read its bounded entry headers without inflating files; the kernel validates and
/// extracts each zlib stream. Refuse a camera-only or truncated archive here, before it can shadow the seed's plugin.
fn contains_metal(blob: &[u8]) -> bool {
    fn up8(n: usize) -> Option<usize> { n.checked_add(7).map(|v| v & !7) }
    if blob.len() < 24 || blob.len() > 64 << 20 || blob.get(..8) != Some(b"NGPUADD2".as_slice())
        || le::try_u64(blob, 16) != Some(blob.len() as u64)
    {
        return false;
    }
    let Some(count) = le::try_u32(blob, 8).map(|n| n as usize).filter(|n| (1..=4096).contains(n)) else { return false };
    let (mut at, mut executable, mut vulkan, mut publish) = (24usize, false, false, false);
    for _ in 0..count {
        let (Some(name_len), Some(file_len), Some(packed_len)) = (
            le::try_u32(blob, at).map(|n| n as usize),
            at.checked_add(8).and_then(|i| le::try_u64(blob, i)).and_then(|n| usize::try_from(n).ok()),
            at.checked_add(16).and_then(|i| le::try_u64(blob, i)).and_then(|n| usize::try_from(n).ok()),
        ) else { return false };
        if !(2..=160).contains(&name_len) { return false; }
        let Some(name_start) = at.checked_add(24) else { return false };
        let Some(name_area) = up8(name_len) else { return false };
        let Some(name_end) = name_start.checked_add(name_area) else { return false };
        let Some(name_bytes) = blob.get(name_start..name_start + name_len - 1) else { return false };
        if blob.get(name_start + name_len - 1) != Some(&0) { return false; }
        let Ok(name) = std::str::from_utf8(name_bytes) else { return false };
        executable |= name == "addons/NativeAdrenoMetal.bundle/NativeAdrenoMetal" && file_len > 0 && packed_len > 0;
        vulkan |= name == "addons/NativeAdrenoMetal.bundle/libvulkan_freedreno.dylib" && file_len > 0 && packed_len > 0;
        publish |= name == "addons/NativeAdrenoMetal.publish" && file_len > 0 && packed_len > 0;
        let Some(compressed_area) = up8(packed_len) else { return false };
        let Some(end) = name_end.checked_add(compressed_area) else { return false };
        if end > blob.len() { return false; }
        at = end;
    }
    at == blob.len() && executable && vulkan && publish
}

/// `ios addon`'s CDHash list (and `phone addon`'s reader of it): one 40-hex-digit CDHash per line, then the file's
/// name; blank lines and `#` comments are skipped.
fn parse_cdhashes(text: &[u8]) -> Result<Vec<[u8; 20]>, String> {
    let text = std::str::from_utf8(text).map_err(|_| "not a text list".to_string())?;
    let mut out = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let word = line.split_whitespace().next().unwrap_or("");
        if word.len() != 40 || !word.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("{line:?} is not a CDHash line (40 hex digits, then the file's name)"));
        }
        let mut hash = [0u8; 20];
        for (i, byte) in hash.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&word[2 * i..2 * i + 2], 16)
                .map_err(|_| format!("{word:?} is not a CDHash (hex digits only)"))?;
        }
        out.push(hash);
    }
    Ok(out)
}
