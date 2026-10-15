//! Parallel execution of record-local scripts over an ordered chunk stream.
//!
//! One thread reads the input and cuts it into numbered chunks of whole records, a pool of
//! workers runs the script on chunks independently, and the calling thread puts the results
//! back in order and writes them. The output is byte for byte what the sequential run
//! produces; only scripts proven record-local by [`crate::analysis`] come here.
//!
//! The number of chunks in flight is bounded by credits, so a slow chunk cannot make the
//! others pile up in memory: the reader takes a credit before it reads a chunk and the writer
//! gives it back after the chunk is written.

use std::fs::File;
use std::mem::ManuallyDrop;
use std::os::fd::FromRawFd;
use std::sync::mpsc::{channel, sync_channel};
use std::sync::{Arc, Mutex};

use crate::io::{strerror, Outputs};
use crate::runtime::{run_chunk, ChunkOutput, Options};
use crate::stream::{ChunkReader, OrderedMerger};
use crate::syntax::{parse, Chunk, ParseOptions};
use crate::Fatal;

/// A piece of work for a worker.
enum Work {
    Chunk { seq: u64, first_line: u64, data: Vec<u8> },
    /// The reader failed after the chunks before `seq`.
    Failed { seq: u64, error: Fatal },
}

type Done = (u64, Result<ChunkOutput, Fatal>);

/// Runs the script over `files` (`-` and an empty list mean standard input) with `jobs`
/// workers, writing to `outputs.stdout` in input order.
pub fn run(
    script_chunks: Vec<(Chunk, ParseOptions)>,
    options: &Options,
    files: &[String],
    jobs: usize,
    block: usize,
    outputs: &mut Outputs,
) -> Result<(), Fatal> {
    let jobs = jobs.max(1);
    let script_chunks = Arc::new(script_chunks);
    let credits = jobs * 4;
    let (credit_tx, credit_rx) = sync_channel::<()>(credits);
    for _ in 0..credits {
        credit_tx.send(()).expect("room for every credit");
    }
    let (work_tx, work_rx) = sync_channel::<Work>(credits);
    let work_rx = Arc::new(Mutex::new(work_rx));
    let (done_tx, done_rx) = channel::<Done>();

    let separator = options.separator;
    let mut result = Ok(());
    std::thread::scope(|scope| {
        // The reader: files in order, chunks in order.
        let names: Vec<String> = if files.is_empty() { vec!["-".to_string()] } else { files.to_vec() };
        scope.spawn(move || {
            let mut seq = 0u64;
            let mut first_line = 1u64;
            for name in names {
                let reader: Box<dyn std::io::Read> = if name == "-" {
                    // SAFETY: descriptor 0 is owned by the process; ManuallyDrop keeps it open.
                    Box::new(StdinRead(ManuallyDrop::new(unsafe { File::from_raw_fd(0) })))
                } else {
                    match File::open(&name) {
                        Ok(f) => Box::new(f),
                        Err(e) => {
                            let error = Fatal::new(format!("can't read {name}: {}", strerror(&e)), 2);
                            let _ = work_tx.send(Work::Failed { seq, error });
                            return;
                        }
                    }
                };
                let mut chunks = ChunkReader::new(reader, separator, block);
                loop {
                    match chunks.next_chunk() {
                        Ok(Some(chunk)) => {
                            // Wait until the writer has caught up enough (bounds memory).
                            if credit_rx.recv().is_err() {
                                return; // the writer stopped
                            }
                            let work = Work::Chunk { seq, first_line, data: chunk.data };
                            first_line += chunk.records;
                            seq += 1;
                            if work_tx.send(work).is_err() {
                                return;
                            }
                        }
                        Ok(None) => break,
                        Err(e) => {
                            let error = Fatal::new(format!("read error on {}: {}", crate::io::display_name(&name), strerror(&e)), 4);
                            let _ = work_tx.send(Work::Failed { seq, error });
                            return;
                        }
                    }
                }
            }
        });

        // The workers.
        for _ in 0..jobs {
            let work_rx = Arc::clone(&work_rx);
            let done_tx = done_tx.clone();
            let script_chunks = Arc::clone(&script_chunks);
            let options = options.clone();
            scope.spawn(move || {
                // The script holds shared pointers, so each worker parses its own copy.
                let Ok(script) = parse(&script_chunks) else { return };
                loop {
                    let work = work_rx.lock().expect("work queue").recv();
                    let Ok(work) = work else { return };
                    let done = match work {
                        Work::Chunk { seq, first_line, data } => (seq, run_chunk(&script, &options, data, first_line)),
                        Work::Failed { seq, error } => (seq, Err(error)),
                    };
                    if done_tx.send(done).is_err() {
                        return;
                    }
                }
            });
        }
        drop(done_tx);

        // The writer: results in order.
        let mut merger = OrderedMerger::default();
        'receive: for (seq, output) in done_rx.iter() {
            merger.push(seq, output);
            while let Some(output) = merger.next_ready() {
                match output {
                    Ok(chunk) => {
                        if !chunk.bytes.is_empty() {
                            if let Err(e) = outputs.stdout.write_raw(&chunk.bytes) {
                                result = Err(crate::io::write_error("stdout", &e));
                                break 'receive;
                            }
                            outputs.stdout.set_unterminated(chunk.unterminated);
                        }
                        let _ = credit_tx.send(());
                    }
                    Err(error) => {
                        result = Err(error);
                        break 'receive;
                    }
                }
            }
        }
        // Dropping the receivers ends the reader and the workers.
        drop(done_rx);
        drop(credit_tx);
    });
    result
}

/// File descriptor 0 read directly, never closed.
struct StdinRead(ManuallyDrop<File>);

impl std::io::Read for StdinRead {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::ChunkSource;
    use std::io::Write;

    fn script(text: &str) -> Vec<(Chunk, ParseOptions)> {
        vec![(Chunk { source: ChunkSource::Expression(1), text: text.as_bytes().to_vec() }, ParseOptions::default())]
    }

    /// A file that cannot be opened (the caller has checked it earlier, but it may have gone away).
    #[test]
    fn a_file_that_cannot_be_read_ends_the_run() {
        let mut outputs = Outputs::new(Box::new(std::io::sink()), b'\n');
        let files = vec!["/nonexistent-directory/input".to_string()];
        let error = run(script("s/a/b/"), &Options::default(), &files, 2, 64, &mut outputs).unwrap_err();
        assert_eq!(error.status, 2);
        assert_eq!(error.message, "can't read /nonexistent-directory/input: No such file or directory");
    }

    struct Failing;

    impl Write for Failing {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from_raw_os_error(28))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// When the writer stops on an error, the reader waiting for a credit stops too.
    #[test]
    fn the_reader_stops_when_the_writer_does() {
        let dir = std::env::temp_dir().join(format!("ripgsed-parallel-unit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("many-lines");
        std::fs::write(&path, "some text\n".repeat(5000)).unwrap();
        let mut outputs = Outputs::new(Box::new(Failing), b'\n');
        outputs.unbuffered = true;
        let files = vec![path.to_string_lossy().into_owned()];
        let result = run(script("s/some/any/"), &Options::default(), &files, 1, 16, &mut outputs);
        assert!(result.is_err());
        assert!(Failing.flush().is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
