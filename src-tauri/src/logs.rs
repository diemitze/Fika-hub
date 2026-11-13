use serde::Serialize;
use std::collections::VecDeque;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const SERVER: usize = 0;
pub const HEADLESS: usize = 1;
pub const GAME: usize = 2;

const CAP: usize = 6000;

/// The UI splits a stream into runs at this line, so counts and history are per run.
pub const RUN_MARK: &str = "[Fika Hub] New run: ";

#[derive(Default)]
struct Stream {
    lines: VecDeque<(u64, String)>,
    next: u64,
    pending: String,
}

#[derive(Default)]
pub struct Logs {
    streams: [Mutex<Stream>; 3],
}

#[derive(Serialize)]
pub struct Chunk {
    pub lines: Vec<(u64, String)>,
    /// Caller fell behind the ring buffer, so it should drop what it shows.
    pub reset: bool,
}

pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' {
            if it.peek() == Some(&'[') {
                it.next();
                while let Some(&n) = it.peek() {
                    it.next();
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        if c.is_control() && c != '\t' {
            continue;
        }
        out.push(c);
    }
    out
}

impl Logs {
    pub fn push_line(&self, idx: usize, line: String) {
        let mut s = self.streams[idx].lock().unwrap();
        let seq = s.next;
        s.next += 1;
        s.lines.push_back((seq, line));
        while s.lines.len() > CAP {
            s.lines.pop_front();
        }
    }

    pub fn push_text(&self, idx: usize, text: &str) {
        let complete: Vec<String> = {
            let mut s = self.streams[idx].lock().unwrap();
            s.pending.push_str(&text.replace("\r\n", "\n").replace('\r', "\n"));
            let mut out = vec![];
            while let Some(i) = s.pending.find('\n') {
                let line: String = s.pending.drain(..=i).collect();
                out.push(strip_ansi(line.trim_end_matches('\n')));
            }
            out
        };
        for l in complete {
            self.push_line(idx, l);
        }
    }

    pub fn since(&self, idx: usize, after: i64) -> Chunk {
        let s = self.streams[idx].lock().unwrap();
        let first = s.lines.front().map(|l| l.0 as i64).unwrap_or(s.next as i64);
        let reset = after >= 0 && after + 1 < first || after >= s.next as i64;
        let from = if reset { -1 } else { after };
        let mut lines: Vec<(u64, String)> = s.lines.iter().filter(|l| l.0 as i64 > from).cloned().collect();
        if lines.len() > 3000 {
            lines.drain(..lines.len() - 3000);
        }
        Chunk { lines, reset }
    }

    pub fn dump(&self, idx: usize) -> String {
        let s = self.streams[idx].lock().unwrap();
        s.lines.iter().map(|l| l.1.as_str()).collect::<Vec<_>>().join("\n")
    }

    /// The newest error-looking line in the current run, for crash alerts.
    pub fn last_error(&self, idx: usize) -> Option<String> {
        let s = self.streams[idx].lock().unwrap();
        s.lines
            .iter()
            .rev()
            .take(400)
            .take_while(|l| !l.1.starts_with(RUN_MARK))
            .map(|l| l.1.as_str())
            .find(|l| {
                let low = l.to_ascii_lowercase();
                !l.starts_with("[Fika Hub]")
                    && (low.contains("exception") || low.contains("[error") || low.contains("[fatal") || low.contains("[critical"))
            })
            .map(|l| {
                // drop the timestamp and level brackets, keep the message
                let msg = l.rsplit("] ").next().unwrap_or(l).trim();
                if msg.chars().count() > 110 {
                    msg.chars().take(107).collect::<String>() + "..."
                } else {
                    msg.to_string()
                }
            })
    }

    pub fn mark_run(&self, idx: usize, why: &str) {
        self.push_line(idx, format!("{RUN_MARK}{why}"));
    }

    pub fn clear(&self, idx: usize) {
        let mut s = self.streams[idx].lock().unwrap();
        s.lines.clear();
        s.pending.clear();
    }
}

/// Follows a log another process writes. Starts at the end of the file so an old
/// session isn't dumped in, and notices when BepInEx recreates the file on game
/// start by watching its first bytes (which carry a timestamp).
pub fn tail(logs: Arc<Logs>, idx: usize, path_fn: impl Fn() -> Option<PathBuf> + Send + 'static) {
    std::thread::spawn(move || {
        use std::os::windows::fs::OpenOptionsExt;
        let mut cur: Option<PathBuf> = None;
        let mut pos: Option<u64> = None;
        let mut head: Vec<u8> = vec![];

        loop {
            std::thread::sleep(Duration::from_millis(400));
            let p = path_fn();
            if p != cur {
                cur = p;
                pos = None;
                head.clear();
            }
            let Some(path) = &cur else { continue };
            let Ok(mut f) = std::fs::OpenOptions::new().read(true).share_mode(7).open(path) else {
                continue;
            };
            let len = f.metadata().map(|m| m.len()).unwrap_or(0);

            let mut new_head = vec![0u8; 96.min(len as usize)];
            if f.read_exact(&mut new_head).is_err() {
                new_head.clear();
            }
            let recreated = !head.is_empty() && !new_head.starts_with(&head[..head.len().min(new_head.len())]);
            if head.len() < 96 || recreated {
                head = new_head;
            }

            let start = match pos {
                None => {
                    pos = Some(len);
                    continue;
                }
                Some(_) if recreated => 0,
                Some(x) if len < x => 0,
                Some(x) => x,
            };
            if len <= start {
                pos = Some(start);
                continue;
            }
            let want = (len - start).min(512 * 1024) as usize;
            let mut buf = vec![0u8; want];
            if f.seek(SeekFrom::Start(start)).is_err() {
                continue;
            }
            if start == 0 {
                logs.mark_run(idx, "log file restarted");
            }
            let read = f.read(&mut buf).unwrap_or(0);
            pos = Some(start + read as u64);
            if read > 0 {
                logs.push_text(idx, &String::from_utf8_lossy(&buf[..read]));
            }
        }
    });
}
