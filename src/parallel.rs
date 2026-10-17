//! Parallel execution of record-local scripts over an ordered chunk stream.
//!
//! One thread reads the input and cuts it into numbered chunks of whole records, a pool of
//! workers runs the script on chunks independently, and the calling thread puts the results
//! back in order and writes them. The output is byte for byte what the sequential run
//! produces; only scripts proven record-local by [`crate::analysis`] come here.
//!
//! `q` and `Q` stop the run at the first chunk, in input order, whose records reach them: the writer
//! writes that chunk up to the record, keeps its exit status, and drops everything after it. A
//! worker that learns of a quit skips the chunks after it, and the reader stops reading.
//!
//! The number of chunks in flight is bounded by credits, so a slow chunk cannot make the
//! others pile up in memory: the reader takes a credit before it reads a chunk and the writer
//! gives it back after the chunk is written.

use std::fs::File;
use std::collections::VecDeque;
use std::io::{Seek, SeekFrom};
use std::mem::ManuallyDrop;
use std::os::fd::FromRawFd;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, sync_channel};
use std::sync::{Arc, Mutex};

use crate::io::{strerror, Outputs};
use crate::runtime::{run_chunk, uses_ranges, Carry, ChunkInput, ChunkOutput, Options};
use crate::stream::{ChunkReader, OrderedMerger};
use crate::syntax::{parse, Chunk, ParseOptions, Script};
use crate::Fatal;

/// A chunk and where standard input stands after it, when the chunk comes from it and it can seek.
struct Job {
    input: ChunkInput,
    stdin_end: Option<u64>,
}

/// A piece of work for a worker.
enum Work {
    Chunk { seq: u64, job: Job },
    /// The reader failed after the chunks before `seq`.
    Failed { seq: u64, error: Fatal },
}

/// What running a chunk gave: the output and, when the run quit in the chunk, where standard input
/// stands just after the record it quit on.
struct Ran {
    output: ChunkOutput,
    stdin_offset: Option<u64>,
    /// The input, when the chunk starts a file that is a stream of its own.
    new_file: Option<Arc<str>>,
}

/// What a worker returns for a chunk. A script with ranges is run on a guess of the state the chunk
/// starts in: `guess` is the guess and `redo` is the chunk, kept to run it again if the chunk before it
/// ends in another state.
struct Done {
    seq: u64,
    ran: Result<Ran, Fatal>,
    guess: Option<Carry>,
    redo: Option<Job>,
}

fn run_job(script: &Script, options: &Options, job: &Job, carry: Option<&Carry>) -> Result<Ran, Fatal> {
    let output = run_chunk(script, options, &job.input, carry)?;
    // The end of the chunk, less what the run did not read of it.
    let stdin_offset = match (&output.quit, job.stdin_end) {
        (Some(quit), Some(end)) => Some(end - (job.input.data.len() - quit.consumed) as u64),
        _ => None,
    };
    let new_file = job.input.first_of_file.then(|| Arc::clone(&job.input.name));
    Ok(Ran { output, stdin_offset, new_file })
}

/// Where the output of a chunk goes.
pub enum Target<'a> {
    /// Standard output.
    Main,
    /// The file named by `w`.
    File(&'a [u8]),
    /// The writer starts the output for the input of that name (`-i`: the file that replaces it).
    NewFile(&'a str),
    /// The writer has taken the last chunk: the file edited in place is finished.
    EndFile,
    /// The run failed: the file edited in place is left as it was.
    Abort,
}

/// What a run leaves behind: the exit status (a `q` or `Q` may ask for one) and where to put the offset
/// of standard input back to, when it quit early.
pub struct Finish {
    pub status: i32,
    pub stdin_offset: Option<u64>,
}

/// Where the reader thread gets its bytes.
type Reader<'a> = Box<dyn std::io::Read + Send + 'a>;

/// Opens the inputs one after another; an error is reported as the message of the run.
type Open<'a> = Box<dyn FnMut() -> Option<Result<Input<'a>, Fatal>> + Send + 'a>;

/// An input that was opened: its name, its bytes, and the offset descriptor 0 starts at when it is
/// standard input that can seek (so that a quit can leave it where the sequential run would).
struct Input<'a> {
    name: String,
    reader: Reader<'a>,
    stdin_start: Option<u64>,
}

/// Runs the script over `files` (`-` and an empty list mean standard input) with `jobs`
/// workers, writing to `outputs.stdout` in input order. Returns the exit status; a run that quit
/// leaves standard input, when it can seek, after the last record it used.
pub fn run(
    script_chunks: Vec<(Chunk, ParseOptions)>,
    options: &Options,
    files: &[String],
    jobs: usize,
    block: usize,
    outputs: &mut Outputs,
) -> Result<i32, Fatal> {
    let mut names = if files.is_empty() { vec!["-".to_string()] } else { files.to_vec() }.into_iter();
    let open: Open<'static> = Box::new(move || {
        let name = names.next()?;
        let mut stdin_start = None;
        let reader: Reader<'static> = if name == "-" {
            // SAFETY: descriptor 0 is owned by the process; ManuallyDrop keeps it open.
            let mut file = ManuallyDrop::new(unsafe { File::from_raw_fd(0) });
            #[allow(clippy::seek_from_current)]
            {
                stdin_start = file.seek(SeekFrom::Current(0)).ok();
            }
            Box::new(StdinRead(file))
        } else {
            match File::open(&name) {
                Ok(f) => Box::new(f),
                Err(e) => return Some(Err(Fatal::new(format!("can't read {name}: {}", strerror(&e)), 2))),
            }
        };
        Some(Ok(Input { name, reader, stdin_start }))
    });
    // (`-i`: the writer begins and finishes the file that replaces each input, as the sequential run does.)
    let backup = options.in_place.clone();
    let follow_symlinks = options.follow_symlinks;
    let finish = run_inputs(script_chunks, options, open, jobs, block, &mut |target: Target, bytes: &[u8], unterminated: bool| {
        let output = match target {
            Target::Main => outputs.main(),
            Target::File(name) => {
                outputs.open(name)?;
                outputs.get(name)
            }
            Target::NewFile(name) => {
                outputs.finish_in_place(backup.as_deref())?;
                return outputs.begin_in_place(name, follow_symlinks);
            }
            Target::EndFile => return outputs.finish_in_place(backup.as_deref()),
            Target::Abort => {
                outputs.abort_in_place();
                return Ok(());
            }
        };
        output.write_raw(bytes).map_err(|e| crate::io::write_error("stdout", &e))?;
        output.set_unterminated(unterminated);
        Ok(())
    })?;
    if let Some(offset) = finish.stdin_offset {
        // The reader has stopped: nothing moves descriptor 0 any more.
        // SAFETY: as above.
        let mut file = ManuallyDrop::new(unsafe { File::from_raw_fd(0) });
        let _ = file.seek(SeekFrom::Start(offset));
    }
    Ok(finish.status)
}

/// Runs the script over one reader with `jobs` workers; `write` gets the output of each chunk,
/// in input order, with whether it ends without a terminator. Returns the exit status.
pub fn run_reader<'a>(
    script_chunks: Vec<(Chunk, ParseOptions)>,
    options: &Options,
    reader: Reader<'a>,
    jobs: usize,
    block: usize,
    write: &mut dyn FnMut(&[u8], bool) -> Result<(), Fatal>,
) -> Result<i32, Fatal> {
    let mut reader = Some(reader);
    let open: Open<'a> = Box::new(move || reader.take().map(|reader| Ok(Input { name: "-".to_string(), reader, stdin_start: None })));
    // (A script that writes files does not come here: see `Run::run_parallel`.)
    let mut write_main = |target: Target, bytes: &[u8], unterminated: bool| match target {
        Target::Main => write(bytes, unterminated),
        Target::File(_) | Target::NewFile(_) | Target::EndFile | Target::Abort => Ok(()),
    };
    run_inputs(script_chunks, options, open, jobs, block, &mut write_main).map(|finish| finish.status)
}

fn run_inputs<'a>(
    script_chunks: Vec<(Chunk, ParseOptions)>,
    options: &Options,
    mut open: Open<'a>,
    jobs: usize,
    block: usize,
    write: &mut dyn FnMut(Target, &[u8], bool) -> Result<(), Fatal>,
) -> Result<Finish, Fatal> {
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
    // The first chunk a worker has seen the run quit in, so that the others can leave the chunks after it.
    let quit_seq = Arc::new(AtomicU64::new(u64::MAX));
    // The state of the ranges that the last chunk the writer took ended in: the workers start from it.
    let hint = Arc::new(Mutex::new(None::<Carry>));
    // Where descriptor 0 stood when the reader first opened it as an input.
    let stdin_start = Arc::new(Mutex::new(None::<u64>));

    let separator = options.separator;
    let separate = options.separate;
    let in_place = options.in_place.is_some();
    // How many records after a chunk the script has to see before the chunk is run (at least one, for `$`).
    let needed = parse(&script_chunks).map_or(1, |script| crate::analysis::lookahead(&script));
    let mut result = Ok(Finish { status: 0, stdin_offset: None });
    std::thread::scope(|scope| {
        // The reader: inputs in order, chunks in order.
        scope.spawn({
            let quit_seq = Arc::clone(&quit_seq);
            let stdin_start = Arc::clone(&stdin_start);
            move || {
                let mut seq = 0u64;
                let mut first_line = 1u64;
                let per_file = separate || in_place;
                // A chunk is held back until `needed` records are read after it (or the inputs end), so that it
                // can say how many records follow it: `$` is the last record of everything that is read, not of
                // one input, and `$-N` needs to see N records ahead. `behind` is how many records the held
                // chunks hold apart from the first.
                let mut held: VecDeque<Work> = VecDeque::new();
                let mut behind = 0u64;
                // Sends the chunks that have enough after them (all of them with `flush`), each when the
                // writer has caught up enough (bounds memory). `false` if the writer stopped.
                let release = |held: &mut VecDeque<Work>, behind: &mut u64, flush: bool| {
                    while !held.is_empty() && (flush || *behind >= needed) {
                        if credit_rx.recv().is_err() {
                            return false;
                        }
                        let mut work = held.pop_front().expect("a held chunk");
                        if let Work::Chunk { job, .. } = &mut work {
                            job.input.after = (*behind).min(needed);
                        }
                        *behind = match held.front() {
                            Some(Work::Chunk { job, .. }) => *behind - job.input.records,
                            _ => 0,
                        };
                        if work_tx.send(work).is_err() {
                            return false;
                        }
                    }
                    true
                };
                while let Some(next) = open() {
                    let Input { name, reader, stdin_start: start } = match next {
                        Ok(input) => input,
                        Err(error) => {
                            if release(&mut held, &mut behind, true) {
                                let _ = work_tx.send(Work::Failed { seq, error });
                            }
                            return;
                        }
                    };
                    if let Some(start) = start {
                        stdin_start.lock().expect("stdin start").get_or_insert(start);
                    }
                    // Each file is a stream of its own with `-s` and `-i`: the chunks held from the one before know
                    // all that follows them, and the numbers of the lines start again.
                    if per_file {
                        if !release(&mut held, &mut behind, true) {
                            return;
                        }
                        first_line = 1;
                    }
                    let mut first_of_file = per_file;
                    let name: Arc<str> = Arc::from(name);
                    let mut stdin_end = start;
                    let mut chunks = ChunkReader::new(reader, separator, block);
                    loop {
                        // Nothing after a quit is used: do not read it.
                        if quit_seq.load(Ordering::Relaxed) < seq {
                            return;
                        }
                        match chunks.next_chunk() {
                            Ok(Some(chunk)) => {
                                stdin_end = stdin_end.map(|end| end + chunk.data.len() as u64);
                                let input = ChunkInput { data: chunk.data, first_line, first_of_file, records: chunk.records, after: 0, name: Arc::clone(&name) };
                                first_of_file = false;
                                first_line += chunk.records;
                                seq += 1;
                                if !held.is_empty() {
                                    behind += chunk.records;
                                }
                                held.push_back(Work::Chunk { seq: seq - 1, job: Job { input, stdin_end } });
                                if !release(&mut held, &mut behind, false) {
                                    return;
                                }
                            }
                            Ok(None) => break,
                            Err(e) => {
                                let error = Fatal::new(format!("read error on {}: {}", crate::io::display_name(&name), strerror(&e)), 4);
                                if release(&mut held, &mut behind, true) {
                                    let _ = work_tx.send(Work::Failed { seq, error });
                                }
                                return;
                            }
                        }
                    }
                }
                release(&mut held, &mut behind, true);
            }
        });

        // The workers.
        for _ in 0..jobs {
            let work_rx = Arc::clone(&work_rx);
            let done_tx = done_tx.clone();
            let script_chunks = Arc::clone(&script_chunks);
            let quit_seq = Arc::clone(&quit_seq);
            let hint = Arc::clone(&hint);
            let options = options.clone();
            scope.spawn(move || {
                // The script holds shared pointers, so each worker parses its own copy.
                let Ok(script) = parse(&script_chunks) else { return };
                let spread_ranges = uses_ranges(&script);
                loop {
                    let work = work_rx.lock().expect("work queue").recv();
                    let Ok(work) = work else { return };
                    let done = match work {
                        // The writer never gets to a chunk after a quit: leave it empty.
                        Work::Chunk { seq, .. } if quit_seq.load(Ordering::Relaxed) < seq => {
                            let output = ChunkOutput { bytes: Vec::new(), unterminated: false, quit: None, carry: None, files: Vec::new() };
                            Done { seq, ran: Ok(Ran { output, stdin_offset: None, new_file: None }), guess: None, redo: None }
                        }
                        Work::Chunk { seq, job } => {
                            // The state the chunk before it ended in, as far as anyone knows.
                            let guess = if spread_ranges && !job.input.first_of_file { hint.lock().expect("carry hint").clone() } else { None };
                            let ran = run_job(&script, &options, &job, guess.as_ref());
                            // (A quit under a wrong guess is not one: only the writer knows.)
                            if !spread_ranges && ran.as_ref().is_ok_and(|ran| ran.output.quit.is_some()) {
                                quit_seq.fetch_min(seq, Ordering::Relaxed);
                            }
                            Done { seq, ran, guess, redo: spread_ranges.then_some(job) }
                        }
                        Work::Failed { seq, error } => Done { seq, ran: Err(error), guess: None, redo: None },
                    };
                    if done_tx.send(done).is_err() {
                        return;
                    }
                }
            });
        }
        drop(done_tx);

        // The writer: results in order. A chunk run on a wrong guess is run again here, on the state
        // the chunk before it ended in.
        let mut merger = OrderedMerger::default();
        let mut carry: Option<Carry> = None;
        let mut own_script = None;
        'receive: for done in done_rx.iter() {
            merger.push(done.seq, done);
            while let Some(done) = merger.next_ready() {
                let Done { ran, guess, redo, .. } = done;
                let mut ran = match ran {
                    Ok(ran) => ran,
                    Err(error) => {
                        result = Err(error);
                        break 'receive;
                    }
                };
                if ran.new_file.is_some() {
                    // A file that is a stream of its own starts from the state of the start of an input.
                    carry = None;
                }
                if let Some(job) = redo {
                    if guess != carry {
                        let script = match &own_script {
                            Some(script) => script,
                            None => match parse(&script_chunks) {
                                Ok(script) => own_script.insert(script),
                                Err(_) => {
                                    result = Err(Fatal::new("internal error: the script does not parse again", 4));
                                    break 'receive;
                                }
                            },
                        };
                        ran = match run_job(script, options, &job, carry.as_ref()) {
                            Ok(ran) => ran,
                            Err(error) => {
                                result = Err(error);
                                break 'receive;
                            }
                        };
                    }
                    carry = ran.output.carry.clone();
                    *hint.lock().expect("carry hint") = carry.clone();
                }
                let Ran { output: chunk, stdin_offset, new_file } = ran;
                if let (Some(name), true) = (&new_file, in_place) {
                    if let Err(e) = write(Target::NewFile(name), &[], false) {
                        result = Err(e);
                        break 'receive;
                    }
                }
                if !chunk.bytes.is_empty() {
                    if let Err(e) = write(Target::Main, &chunk.bytes, chunk.unterminated) {
                        result = Err(e);
                        break 'receive;
                    }
                }
                for (name, bytes, unterminated) in &chunk.files {
                    if let Err(e) = write(Target::File(name), bytes, *unterminated) {
                        result = Err(e);
                        break 'receive;
                    }
                }
                if let Some(quit) = chunk.quit {
                    // What the reader opened after a quit in an earlier input is not used: standard input
                    // goes back to where it was when it was opened.
                    let offset = stdin_offset.or(*stdin_start.lock().expect("stdin start"));
                    result = Ok(Finish { status: quit.code, stdin_offset: offset });
                    break 'receive;
                }
                let _ = credit_tx.send(());
            }
        }
        // The file edited in place is finished, or left as it was if the run failed.
        if in_place {
            let end = write(if result.is_ok() { Target::EndFile } else { Target::Abort }, &[], false);
            if let (Ok(_), Err(e)) = (&result, end) {
                result = Err(e);
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
