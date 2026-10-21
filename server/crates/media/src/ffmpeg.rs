//! One way to start ffmpeg: [`Ffmpeg`] sets up the command the way every caller wants it (stdin closed
//! unless piped, killed when its handle is dropped, optionally at low priority) and leaves the arguments
//! to the caller, in the order ffmpeg needs them.
use std::ffi::OsStr;
use std::process::Stdio;

/// An ffmpeg run being put together. `Ffmpeg::new().quiet().seek(t).input(path).args([...])` gives
/// `ffmpeg -v error -nostdin -ss <t> -i <path> ...`.
pub struct Ffmpeg {
    cmd: tokio::process::Command,
}

impl Default for Ffmpeg {
    fn default() -> Self {
        Self::new()
    }
}

impl Ffmpeg {
    /// `ffmpeg` with no arguments yet, stdin closed, killed when its handle (or its output future) is dropped.
    /// stdout and stderr are left to the caller: `output()` collects both, `spawn()` inherits them unless set.
    pub fn new() -> Self {
        Self::program("ffmpeg")
    }

    /// `ffprobe`, set up the same way.
    pub fn probe() -> Self {
        Self::program("ffprobe")
    }

    fn program(name: &str) -> Self {
        let mut cmd = tokio::process::Command::new(name);
        cmd.kill_on_drop(true).stdin(Stdio::null());
        Ffmpeg { cmd }
    }

    /// `-v error -nostdin`: only errors in the log, never reads the terminal (the usual start of a run whose
    /// answer is audio).
    pub fn quiet(self) -> Self {
        self.args(["-v", "error", "-nostdin"])
    }

    /// `-ss <secs>` (two decimals) when `secs` is past the start; before `input` it is a fast input seek.
    pub fn seek(self, secs: f64) -> Self {
        if secs > 0.0 {
            self.args(["-ss", &format!("{secs:.2}")])
        } else {
            self
        }
    }

    /// `-i <path>`.
    pub fn input(self, path: impl AsRef<OsStr>) -> Self {
        self.arg("-i").arg(path)
    }

    pub fn arg(mut self, a: impl AsRef<OsStr>) -> Self {
        self.cmd.arg(a);
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.cmd.args(args);
        self
    }

    /// At the lowest CPU priority and idle disk priority, so it never competes with playback or downloads
    /// (Linux; elsewhere it runs as usual).
    pub fn low_priority(mut self) -> Self {
        #[cfg(target_os = "linux")]
        unsafe {
            self.cmd.pre_exec(|| {
                libc::setpriority(libc::PRIO_PROCESS, 0, 19);
                encore_jobs::busy::idle_io_for_child();
                Ok(())
            });
        }
        self
    }

    /// [`Ffmpeg::low_priority`] when `low` is true.
    pub fn low_priority_if(self, low: bool) -> Self {
        if low {
            self.low_priority()
        } else {
            self
        }
    }

    /// Audio goes in through stdin.
    pub fn stdin_piped(mut self) -> Self {
        self.cmd.stdin(Stdio::piped());
        self
    }

    /// The answer comes out of stdout; the log is dropped.
    pub fn pipe_out(mut self) -> Self {
        self.cmd.stdout(Stdio::piped()).stderr(Stdio::null());
        self
    }

    /// Neither the output nor the log is wanted (ffmpeg writes a file).
    pub fn silent(mut self) -> Self {
        self.cmd.stdout(Stdio::null()).stderr(Stdio::null());
        self
    }

    /// The command itself, for anything this builder doesn't cover.
    pub fn command(&mut self) -> &mut tokio::process::Command {
        &mut self.cmd
    }

    pub fn spawn(mut self) -> std::io::Result<tokio::process::Child> {
        self.cmd.spawn()
    }

    /// Runs it to the end, collecting stdout and stderr.
    pub async fn output(mut self) -> std::io::Result<std::process::Output> {
        self.cmd.output().await
    }

    pub async fn status(mut self) -> std::io::Result<std::process::ExitStatus> {
        self.cmd.status().await
    }
}

/// `ffmpeg -version` runs (the server keeps the answer: [`crate::Media::ffmpeg_ok`]).
pub fn available() -> bool {
    std::process::Command::new("ffmpeg").arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(f: &mut Ffmpeg) -> Vec<String> {
        let std = f.command().as_std();
        std::iter::once(std.get_program()).chain(std.get_args()).map(|a| a.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn arguments_come_out_in_order() {
        let mut f = Ffmpeg::new().quiet().seek(12.345).input("/m/a.flac").args(["-map", "0:a:0"]);
        assert_eq!(argv(&mut f), ["ffmpeg", "-v", "error", "-nostdin", "-ss", "12.35", "-i", "/m/a.flac", "-map", "0:a:0"]);
        // no seek at the start
        let mut f = Ffmpeg::new().quiet().seek(0.0).input("x");
        assert_eq!(argv(&mut f), ["ffmpeg", "-v", "error", "-nostdin", "-i", "x"]);
        assert_eq!(argv(&mut Ffmpeg::probe().arg("-v")), ["ffprobe", "-v"]);
    }

    /// The nice value a run gets: the background's at 19, a stream's as the server's own.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn background_runs_at_idle_priority_and_streams_at_the_normal_one() {
        let nice = |low: bool| async move {
            // (field 19 of /proc/self/stat is the nice value; `cut` inherits it from the shell)
            let out = Ffmpeg::program("sh").low_priority_if(low).args(["-c", "cut -d' ' -f19 /proc/self/stat"]).pipe_out().spawn().ok()?.wait_with_output().await.ok()?;
            String::from_utf8_lossy(&out.stdout).trim().parse::<i32>().ok()
        };
        let own = std::fs::read_to_string("/proc/self/stat").ok().and_then(|s| s.rsplit(')').next()?.split_whitespace().nth(16)?.parse::<i32>().ok());
        let (Some(low), Some(normal), Some(own)) = (nice(true).await, nice(false).await, own) else { return };
        assert_eq!(low, 19);
        assert_eq!(normal, own, "a stream is not lowered");
    }
}
