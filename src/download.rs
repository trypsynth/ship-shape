use std::{
	fs::{self, File},
	io::{Read, Write},
	path::PathBuf,
	sync::atomic::{AtomicBool, Ordering},
	time::Duration,
};

use minisign_verify::{PublicKey, Signature};

use crate::{UpdateError, UpdaterConfig, http, platform};

const DOWNLOAD_TIMEOUT: Duration = Duration::from_mins(10);
const CHUNK_SIZE: usize = 8192;

/// A partially downloaded or unverified file that is deleted when dropped.
struct TempDownload(PathBuf);

impl Drop for TempDownload {
	fn drop(&mut self) {
		let _ = fs::remove_file(&self.0);
	}
}

/// Download `url`, verify its minisign signature, and return the local path of the verified file.
///
/// The signature is fetched from `signature_url`. `progress_callback(downloaded, total)` is called
/// after each chunk; `total` may be 0 if the server does not send `Content-Length`.
///
/// `cancelled` is polled between network operations and after every chunk read; as soon as it is
/// set to `true` the download stops, any partially written file is removed, and
/// [`UpdateError::Cancelled`] is returned. This keeps cancellation responsive instead of letting a
/// slow or stalled transfer run to completion (or time out) in the background after the user has
/// already dismissed the progress dialog.
///
/// The destination folder depends on the platform and [`UpdaterConfig::install_kind`]. A portable
/// Windows zip or Linux tar.gz lands next to the current executable so that the extraction script
/// can overwrite in place. A Linux `AppImage` lands in a per-user cache directory. Everything else
/// lands in the system temp directory.
///
/// # Errors
///
/// Returns [`UpdateError`] on network failure, HTTP error, I/O error, cancellation, or signature
/// verification failure.
pub fn download_update_file(
	config: &UpdaterConfig,
	url: &str,
	signature_url: &str,
	cancelled: &AtomicBool,
	mut progress_callback: impl FnMut(u64, u64),
) -> Result<PathBuf, UpdateError> {
	let public_key = PublicKey::from_base64(&config.minisign_public_key)
		.map_err(|e| UpdateError::Verification(format!("Invalid public key: {e}")))?;
	let http = http::agent(DOWNLOAD_TIMEOUT);
	let signature_text = http
		.get(signature_url)
		.header("User-Agent", &config.user_agent)
		.call()?
		.body_mut()
		.read_to_string()
		.map_err(|e| UpdateError::Network(format!("Failed to read signature: {e}")))?;
	let signature =
		Signature::decode(&signature_text).map_err(|e| UpdateError::Verification(format!("Invalid signature: {e}")))?;
	check_cancelled(cancelled)?;
	let resp = http.get(url).header("User-Agent", &config.user_agent).call()?;
	let total_size = resp
		.headers()
		.get("Content-Length")
		.and_then(|v| v.to_str().ok())
		.and_then(|v| v.parse::<u64>().ok())
		.unwrap_or(0);
	let file_name = url.rsplit('/').next().unwrap_or("update.bin");
	let final_path = platform::download_dir(config)?.join(file_name);
	let temp = TempDownload(final_path.with_file_name(format!("{file_name}.tmp")));
	let mut file = File::create(&temp.0).map_err(|e| UpdateError::Io(format!("Failed to create file: {e}")))?;
	let mut downloaded: u64 = 0;
	let mut buffer = [0u8; CHUNK_SIZE];
	let mut body = resp.into_body();
	let mut reader = body.as_reader();
	loop {
		check_cancelled(cancelled)?;
		let n = reader.read(&mut buffer).map_err(|e| UpdateError::Network(e.to_string()))?;
		if n == 0 {
			break;
		}
		file.write_all(&buffer[..n]).map_err(|e| UpdateError::Io(format!("Failed to write file: {e}")))?;
		downloaded += n as u64;
		progress_callback(downloaded, total_size);
	}
	drop(file);
	let data = fs::read(&temp.0).map_err(|e| UpdateError::Io(format!("Failed to read file for verification: {e}")))?;
	public_key
		.verify(&data, &signature, true)
		.map_err(|e| UpdateError::Verification(format!("Signature verification failed: {e}")))?;
	fs::rename(&temp.0, &final_path).map_err(|e| UpdateError::Io(format!("Failed to rename verified file: {e}")))?;
	Ok(final_path)
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), UpdateError> {
	if cancelled.load(Ordering::Relaxed) { Err(UpdateError::Cancelled) } else { Ok(()) }
}
