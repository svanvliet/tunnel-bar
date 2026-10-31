use crate::model::LogLine;
use std::collections::{HashMap, VecDeque};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

const MAX_LINES: usize = 2000;
const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;

/// Per-tunnel in-memory ring buffer plus a rotating log file.
pub struct LogStore {
    dir: PathBuf,
    buffers: Mutex<HashMap<String, VecDeque<LogLine>>>,
}

pub fn file_name(id: &str) -> String {
    let safe: String =
        id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' }).collect();
    format!("{safe}.log")
}

impl LogStore {
    pub fn new(dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        Self { dir, buffers: Mutex::new(HashMap::new()) }
    }

    pub fn dir(&self) -> &PathBuf {
        &self.dir
    }

    pub fn push(&self, id: &str, stream: &str, text: &str) -> LogLine {
        let line = LogLine {
            ts: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            stream: stream.to_string(),
            text: text.trim_end().to_string(),
        };
        {
            let mut b = self.buffers.lock().unwrap();
            let q = b.entry(id.to_string()).or_default();
            if q.len() >= MAX_LINES {
                q.pop_front();
            }
            q.push_back(line.clone());
        }
        self.append_file(id, &line);
        line
    }

    fn append_file(&self, id: &str, line: &LogLine) {
        let path = self.dir.join(file_name(id));
        if std::fs::metadata(&path).map(|m| m.len() > MAX_FILE_BYTES).unwrap_or(false) {
            let _ = std::fs::rename(&path, path.with_extension("log.1"));
        }
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "{} [{}] {}", line.ts, line.stream, line.text);
        }
    }

    pub fn get(&self, id: &str) -> Vec<LogLine> {
        self.buffers.lock().unwrap().get(id).map(|q| q.iter().cloned().collect()).unwrap_or_default()
    }

    pub fn clear(&self, id: &str) {
        self.buffers.lock().unwrap().remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffers_and_writes() {
        let dir = tempfile::tempdir().unwrap();
        let s = LogStore::new(dir.path().to_path_buf());
        for i in 0..(MAX_LINES + 5) {
            s.push("a.usw3", "out", &format!("line {i}"));
        }
        let lines = s.get("a.usw3");
        assert_eq!(lines.len(), MAX_LINES);
        assert_eq!(lines[0].text, "line 5");
        let content = std::fs::read_to_string(dir.path().join("a.usw3.log")).unwrap();
        assert!(content.contains("[out] line 0"));
        assert_eq!(file_name("a/b c"), "a_b_c.log");
    }
}
