//! Moving files between the machine and a container, over exec streams, off the UI thread.
//!
//! - Downloads of folders stream `tar cf -` from the container and extract on the fly; single
//!   files stream `cat`, and large ones are fetched in `dd` chunks appended to a `.kubyl-part`
//!   file, so a retry after a network blip resumes at the last complete chunk.
//! - Uploads stream a local tar archive into `tar xf - -C <dir>` (modes and mtimes kept), or
//!   `cat >` for a single file when the image has no tar.
//! - Verification compares SHA-256 on both sides (`sha256sum` in the container).
//!
//! Progress is reported as byte deltas on a channel; the queue turns them into speed and ETA.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context as _;
use futures::channel::mpsc;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::local;
use crate::remote::{self, RemoteTarget, sh};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Download,
    Upload,
}

/// One file or folder to move.
#[derive(Clone)]
pub struct TransferJob {
    pub direction: Direction,
    pub target: RemoteTarget,
    /// Download: the remote path of the item. Upload: the remote destination directory.
    pub remote: String,
    /// Download: the local destination directory. Upload: the local path of the item.
    pub local: PathBuf,
    /// The name the item gets at the destination (differs for "keep both").
    pub dest_name: String,
    pub is_dir: bool,
    /// Size in bytes if known (0 = unknown), for progress.
    pub size: u64,
    /// Compare SHA-256 after the transfer.
    pub verify: bool,
    /// Files above this size download in resumable chunks.
    pub chunk_size: u64,
}

impl TransferJob {
    /// The destination as shown in the queue.
    pub fn destination(&self) -> String {
        match self.direction {
            Direction::Download => local::display(&self.local.join(&self.dest_name)),
            Direction::Upload => crate::entry::join(&self.remote, &self.dest_name),
        }
    }

    pub fn source_name(&self) -> String {
        match self.direction {
            Direction::Download => crate::entry::file_name(&self.remote).to_string(),
            Direction::Upload => self
                .local
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        }
    }
}

/// How a finished transfer was checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verification {
    Verified,
    Unverified(String),
}

/// Bytes moved since the last report.
pub type ProgressTx = mpsc::UnboundedSender<u64>;

const RETRIES: u32 = 5;

/// Runs `job` to completion.
pub async fn run(job: TransferJob, progress: ProgressTx) -> anyhow::Result<Verification> {
    match (job.direction, job.is_dir) {
        (Direction::Download, true) => download_dir(&job, &progress).await?,
        (Direction::Download, false) => download_file(&job, &progress).await?,
        (Direction::Upload, _) => upload(&job, &progress).await?,
    }
    if !job.verify {
        return Ok(Verification::Unverified("verification is off".into()));
    }
    if !job.target.caps.sha256sum {
        return Ok(Verification::Unverified(
            "no sha256sum in the container".into(),
        ));
    }
    verify(&job).await
}

/// A `Read` over chunks sent from the async side (tar extraction runs on a blocking thread).
struct ChannelReader {
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
    buf: Vec<u8>,
    pos: usize,
}

impl Read for ChannelReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        while self.pos >= self.buf.len() {
            match self.rx.recv() {
                Ok(chunk) => {
                    self.buf = chunk;
                    self.pos = 0;
                }
                Err(_) => return Ok(0),
            }
        }
        let n = out.len().min(self.buf.len() - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// A `Write` that sends chunks to the async side (tar creation runs on a blocking thread).
struct ChannelWriter {
    tx: tokio::sync::mpsc::Sender<Vec<u8>>,
}

impl Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.tx
            .blocking_send(buf.to_vec())
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "transfer stopped"))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn part_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!(".{name}.kubyl-part"))
}

/// Moves a finished download into place, replacing what's there (the conflict was decided
/// before the transfer started).
fn finish(part: &Path, dest: &Path) -> std::io::Result<()> {
    if dest.is_dir() && !part.is_dir() {
        std::fs::remove_dir_all(dest)?;
    }
    std::fs::rename(part, dest)
}

async fn download_file(job: &TransferJob, progress: &ProgressTx) -> anyhow::Result<()> {
    let caps = &job.target.caps;
    remote::require(caps, "Downloading", caps.cat || caps.dd, "cat or dd")?;
    tokio::fs::create_dir_all(&job.local).await?;
    let part = part_path(&job.local, &job.dest_name);
    let dest = job.local.join(&job.dest_name);
    if caps.dd && job.size > job.chunk_size {
        download_chunks(job, &part, progress).await?;
    } else {
        let mut attempt = 0;
        loop {
            match stream_cat(job, &part, progress).await {
                Ok(()) => break,
                Err(err) if attempt < RETRIES => {
                    attempt += 1;
                    tracing::debug!("download retry {attempt}: {err:#}");
                    tokio::time::sleep(backoff(attempt)).await;
                }
                Err(err) => return Err(err),
            }
        }
    }
    finish(&part, &dest).with_context(|| format!("moving the download to {}", dest.display()))?;
    Ok(())
}

fn backoff(attempt: u32) -> Duration {
    Duration::from_secs(1 << attempt.min(4))
}

/// `cat` the whole file into `part` (restarting from zero).
async fn stream_cat(job: &TransferJob, part: &Path, progress: &ProgressTx) -> anyhow::Result<()> {
    let mut process = remote::exec_stream(
        &job.target,
        sh("cat -- \"$1\"", [job.target.real(&job.remote)]),
        false,
    )
    .await?;
    let status = process.take_status();
    let mut stdout = process
        .stdout()
        .ok_or_else(|| anyhow::anyhow!("no output stream"))?;
    let mut stderr = process.stderr();
    let mut file = tokio::fs::File::create(part).await?;
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = stdout.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).await?;
        progress.unbounded_send(n as u64).ok();
    }
    file.flush().await?;
    let mut err_text = String::new();
    if let Some(stderr) = stderr.as_mut() {
        stderr.read_to_string(&mut err_text).await.ok();
    }
    check_status(status, &err_text).await
}

async fn check_status(
    status: Option<
        impl std::future::Future<
            Output = Option<k8s_openapi::apimachinery::pkg::apis::meta::v1::Status>,
        >,
    >,
    stderr: &str,
) -> anyhow::Result<()> {
    let Some(status) = status else { return Ok(()) };
    match status.await {
        Some(s) if s.status.as_deref() == Some("Failure") => {
            let stderr = stderr.trim();
            if stderr.is_empty() {
                anyhow::bail!(s.message.unwrap_or_else(|| "the command failed".into()))
            }
            anyhow::bail!(stderr.lines().last().unwrap_or(stderr).to_string())
        }
        // A dropped connection has no status: the stream ended early.
        None => anyhow::bail!("the connection closed before the command finished"),
        _ => Ok(()),
    }
}

/// Appends `dd` chunks to `part`, resuming after the last complete chunk already there.
async fn download_chunks(
    job: &TransferJob,
    part: &Path,
    progress: &ProgressTx,
) -> anyhow::Result<()> {
    let chunk = job.chunk_size.max(64 * 1024);
    let existing = tokio::fs::metadata(part)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    let mut index = existing / chunk;
    let resume_at = index * chunk;
    let file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(part)
        .await?;
    // Drop a partial last chunk.
    file.set_len(resume_at).await?;
    drop(file);
    if resume_at > 0 {
        progress.unbounded_send(resume_at).ok();
    }
    let mut file = tokio::fs::OpenOptions::new()
        .append(true)
        .open(part)
        .await?;
    let path = job.target.real(&job.remote);
    loop {
        let script = format!("dd if=\"$1\" bs={chunk} skip={index} count=1 2>/dev/null");
        let mut attempt = 0;
        let bytes = loop {
            match job.target.exec(sh(&script, [path.clone()]), None).await {
                Ok(output) if output.success => break output.stdout,
                Ok(output) if attempt >= RETRIES => anyhow::bail!(output.error()),
                Err(err) if attempt >= RETRIES => return Err(err),
                other => {
                    attempt += 1;
                    let reason = match other {
                        Ok(output) => output.error(),
                        Err(err) => format!("{err:#}"),
                    };
                    tracing::debug!(chunk = index, "chunk retry {attempt}: {reason}");
                    tokio::time::sleep(backoff(attempt)).await;
                }
            }
        };
        file.write_all(&bytes).await?;
        progress.unbounded_send(bytes.len() as u64).ok();
        if (bytes.len() as u64) < chunk {
            break;
        }
        index += 1;
    }
    file.flush().await?;
    Ok(())
}

/// `tar cf -` a folder and extract it into a staging directory, then move it into place.
async fn download_dir(job: &TransferJob, progress: &ProgressTx) -> anyhow::Result<()> {
    let caps = &job.target.caps;
    remote::require(caps, "Downloading folders", caps.tar, "tar")?;
    tokio::fs::create_dir_all(&job.local).await?;
    let staging = job.local.join(format!(".{}.kubyl-part", job.dest_name));
    if staging.exists() {
        tokio::fs::remove_dir_all(&staging).await.ok();
    }
    tokio::fs::create_dir_all(&staging).await?;
    let parent = crate::entry::parent(&job.remote);
    let name = crate::entry::file_name(&job.remote).to_string();

    let mut process = remote::exec_stream(
        &job.target,
        sh(
            "cd -- \"$1\" && tar cf - -- \"$2\"",
            [job.target.real(&parent), name.clone()],
        ),
        false,
    )
    .await?;
    let status = process.take_status();
    let mut stdout = process
        .stdout()
        .ok_or_else(|| anyhow::anyhow!("no output stream"))?;
    let mut stderr = process.stderr();

    let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(16);
    let extract_into = staging.clone();
    let extract = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        let reader = ChannelReader {
            rx,
            buf: Vec::new(),
            pos: 0,
        };
        let mut archive = tar::Archive::new(reader);
        archive.set_preserve_permissions(true);
        archive.set_preserve_mtime(true);
        archive.set_overwrite(true);
        archive.unpack(&extract_into)
    });
    let mut buf = vec![0u8; 1 << 16];
    let read_result: anyhow::Result<()> = async {
        loop {
            let n = stdout.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            progress.unbounded_send(n as u64).ok();
            if tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
        Ok(())
    }
    .await;
    drop(tx);
    let extracted = extract.await.context("extracting")?;
    let mut err_text = String::new();
    if let Some(stderr) = stderr.as_mut() {
        stderr.read_to_string(&mut err_text).await.ok();
    }
    read_result?;
    check_status(status, &err_text).await?;
    extracted.context("extracting the archive")?;

    let dest = job.local.join(&job.dest_name);
    let unpacked = staging.join(&name);
    if dest.exists() {
        if dest.is_dir() {
            tokio::fs::remove_dir_all(&dest).await?;
        } else {
            tokio::fs::remove_file(&dest).await?;
        }
    }
    tokio::fs::rename(&unpacked, &dest)
        .await
        .with_context(|| format!("moving the download to {}", dest.display()))?;
    tokio::fs::remove_dir_all(&staging).await.ok();
    Ok(())
}

/// Streams a local file or folder into the container.
async fn upload(job: &TransferJob, progress: &ProgressTx) -> anyhow::Result<()> {
    let caps = job.target.caps.clone();
    if !caps.shell {
        anyhow::bail!("Uploading needs a shell in the container");
    }
    if !caps.tar {
        if job.is_dir {
            anyhow::bail!("Uploading folders needs tar in the container");
        }
        return upload_cat(job, progress).await;
    }
    let mut process = remote::exec_stream(
        &job.target,
        sh(
            "mkdir -p -- \"$1\" && tar xf - -C \"$1\"",
            [job.target.real(&job.remote)],
        ),
        true,
    )
    .await?;
    let status = process.take_status();
    let mut stdin = process
        .stdin()
        .ok_or_else(|| anyhow::anyhow!("no input stream"))?;
    let stderr = process.stderr();
    let stderr_task = tokio::spawn(async move {
        let mut text = String::new();
        if let Some(mut stderr) = stderr {
            stderr.read_to_string(&mut text).await.ok();
        }
        text
    });

    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
    let (source, name, is_dir) = (job.local.clone(), job.dest_name.clone(), job.is_dir);
    let build = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        let writer = std::io::BufWriter::with_capacity(1 << 16, ChannelWriter { tx });
        let mut builder = tar::Builder::new(writer);
        builder.follow_symlinks(false);
        if is_dir {
            builder.append_dir_all(&name, &source)?;
        } else {
            builder.append_path_with_name(&source, &name)?;
        }
        builder.into_inner()?.flush()
    });
    let mut sent = 0u64;
    let total = job.size;
    while let Some(chunk) = rx.recv().await {
        stdin.write_all(&chunk).await?;
        // Tar adds headers and padding: don't report more than the payload.
        let n = chunk.len() as u64;
        let report = if total > 0 {
            n.min(total.saturating_sub(sent))
        } else {
            n
        };
        sent += n;
        progress.unbounded_send(report).ok();
    }
    build
        .await
        .context("archiving")?
        .context("reading the local files")?;
    stdin.shutdown().await.ok();
    drop(stdin);
    let err_text = stderr_task.await.unwrap_or_default();
    check_status(status, &err_text).await
}

/// `cat > dir/name` for a single file (images without tar).
async fn upload_cat(job: &TransferJob, progress: &ProgressTx) -> anyhow::Result<()> {
    let dest = crate::entry::join(&job.remote, &job.dest_name);
    let mut process = remote::exec_stream(
        &job.target,
        sh("cat > \"$1\"", [job.target.real(&dest)]),
        true,
    )
    .await?;
    let status = process.take_status();
    let mut stdin = process
        .stdin()
        .ok_or_else(|| anyhow::anyhow!("no input stream"))?;
    let mut file = tokio::fs::File::open(&job.local).await?;
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        stdin.write_all(&buf[..n]).await?;
        progress.unbounded_send(n as u64).ok();
    }
    stdin.shutdown().await.ok();
    drop(stdin);
    let mut err_text = String::new();
    if let Some(mut stderr) = process.stderr() {
        stderr.read_to_string(&mut err_text).await.ok();
    }
    check_status(status, &err_text).await
}

/// Compares SHA-256 of every file on both sides.
async fn verify(job: &TransferJob) -> anyhow::Result<Verification> {
    let (remote_base, remote_name, local_base, local_name) = match job.direction {
        Direction::Download => (
            crate::entry::parent(&job.remote),
            crate::entry::file_name(&job.remote).to_string(),
            job.local.clone(),
            job.dest_name.clone(),
        ),
        Direction::Upload => (
            job.remote.clone(),
            job.dest_name.clone(),
            job.local
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default(),
            job.local
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        ),
    };
    let remote_hashes = job
        .target
        .sha256_tree(&remote_base, &[remote_name.as_str()])
        .await?;
    let (base, name) = (local_base.clone(), local_name.clone());
    let local_hashes = tokio::task::spawn_blocking(move || local::sha256_tree(&base, &name))
        .await
        .context("hashing")??;
    Ok(compare(
        &remote_hashes,
        &remote_name,
        &local_hashes,
        &local_name,
    ))
}

/// Compares two hash maps whose keys start with different top-level names.
pub fn compare(
    remote: &std::collections::HashMap<String, String>,
    remote_name: &str,
    local: &std::collections::HashMap<String, String>,
    local_name: &str,
) -> Verification {
    if remote.len() != local.len() {
        return Verification::Unverified(format!(
            "{} files in the container, {} here",
            remote.len(),
            local.len()
        ));
    }
    for (path, hash) in remote {
        let local_path = match path.strip_prefix(remote_name) {
            Some(rest) => format!("{local_name}{rest}"),
            None => path.clone(),
        };
        match local.get(&local_path) {
            Some(local_hash) if local_hash == hash => {}
            Some(_) => return Verification::Unverified(format!("{path} differs")),
            None => return Verification::Unverified(format!("{path} is missing")),
        }
    }
    Verification::Verified
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn channel_reader_and_writer_move_bytes() {
        let (tx, rx) = std::sync::mpsc::sync_channel(4);
        tx.send(b"hello ".to_vec()).unwrap();
        tx.send(b"world".to_vec()).unwrap();
        drop(tx);
        let mut reader = ChannelReader {
            rx,
            buf: Vec::new(),
            pos: 0,
        };
        let mut text = String::new();
        reader.read_to_string(&mut text).unwrap();
        assert_eq!(text, "hello world");
    }

    #[test]
    fn tar_round_trip_through_channels() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("certs")).unwrap();
        std::fs::write(dir.path().join("certs/ca.pem"), b"pem").unwrap();
        let out = tempfile::tempdir().unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
        let source = dir.path().join("certs");
        let builder = std::thread::spawn(move || {
            let writer = std::io::BufWriter::new(ChannelWriter { tx });
            let mut builder = tar::Builder::new(writer);
            builder.append_dir_all("certs (1)", &source).unwrap();
            builder.into_inner().unwrap().flush().unwrap();
        });
        let (sync_tx, sync_rx) = std::sync::mpsc::sync_channel(64);
        let target = out.path().to_path_buf();
        let extractor = std::thread::spawn(move || {
            let mut archive = tar::Archive::new(ChannelReader {
                rx: sync_rx,
                buf: Vec::new(),
                pos: 0,
            });
            archive.unpack(&target).unwrap();
        });
        while let Some(chunk) = rx.blocking_recv() {
            sync_tx.send(chunk).unwrap();
        }
        drop(sync_tx);
        builder.join().unwrap();
        extractor.join().unwrap();
        assert_eq!(
            std::fs::read(out.path().join("certs (1)/ca.pem")).unwrap(),
            b"pem"
        );
    }

    #[test]
    fn compares_trees_with_renamed_roots() {
        let remote: HashMap<String, String> = [
            ("certs/a.pem".to_string(), "1".to_string()),
            ("certs/b.pem".to_string(), "2".to_string()),
        ]
        .into();
        let local: HashMap<String, String> = [
            ("certs (1)/a.pem".to_string(), "1".to_string()),
            ("certs (1)/b.pem".to_string(), "2".to_string()),
        ]
        .into();
        assert_eq!(
            compare(&remote, "certs", &local, "certs (1)"),
            Verification::Verified
        );
        let mut changed = local.clone();
        changed.insert("certs (1)/b.pem".into(), "x".into());
        assert!(matches!(
            compare(&remote, "certs", &changed, "certs (1)"),
            Verification::Unverified(_)
        ));
    }
}
