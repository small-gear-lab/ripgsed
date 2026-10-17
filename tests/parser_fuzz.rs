//! The script parser must never panic, whatever the script: it either parses or reports an
//! error. Scripts come from a fixed-seed generator mixing random bytes with sed tokens.
//! `RIPGSED_FUZZ_CASES=1000000 cargo test --test parser_fuzz` runs more cases.

use ripgsed::syntax::{Chunk, ChunkSource, ParseOptions, parse};

/// xorshift64*: a tiny deterministic generator.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const TOKENS: &[&str] = &[
    "s",
    "y",
    "/",
    "\\",
    "a",
    "i",
    "c",
    "b",
    "t",
    "T",
    ":",
    "{",
    "}",
    ";",
    "\n",
    "!",
    ",",
    "$",
    "1",
    "0",
    "2",
    "~",
    "+",
    "p",
    "d",
    "D",
    "n",
    "N",
    "g",
    "G",
    "h",
    "H",
    "x",
    "q",
    "Q",
    "l",
    "=",
    "r",
    "w",
    "R",
    "W",
    "e",
    "v",
    "z",
    "F",
    "#",
    "[",
    "]",
    "[:alpha:]",
    "\\(",
    "\\)",
    "\\{",
    "\\}",
    "*",
    ".",
    "^",
    "&",
    "\\1",
    "\\n",
    "\\t",
    "I",
    "M",
    " ",
    "g",
    "5",
    "label",
    "file.txt",
    "é",
    "\u{0}",
    "\\c",
    "\\x41",
    "\\d065",
    "\\o101",
    "|",
    "?",
    "(",
    ")",
    "{2}",
];

fn script(rng: &mut Rng) -> Vec<u8> {
    let mut out = Vec::new();
    for _ in 0..rng.below(24) {
        if rng.below(8) == 0 {
            out.push(rng.below(256) as u8);
        } else {
            out.extend_from_slice(TOKENS[rng.below(TOKENS.len())].as_bytes());
        }
    }
    out
}

#[test]
fn parsing_never_panics() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let cases: usize = std::env::var("RIPGSED_FUZZ_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20_000);
    for i in 0..cases {
        let chunks: Vec<_> = (0..1 + rng.below(3))
            .map(|n| {
                let options = ParseOptions {
                    extended: rng.below(2) == 0,
                    posix: rng.below(5) == 0,
                    posixly_correct: rng.below(5) == 0,
                    sandbox: rng.below(8) == 0,
                    ..ParseOptions::default()
                };
                let source = if rng.below(4) == 0 {
                    ChunkSource::File("f.sed".into())
                } else {
                    ChunkSource::Expression(n + 1)
                };
                (
                    Chunk {
                        source,
                        text: script(&mut rng),
                    },
                    options,
                )
            })
            .collect();
        let result = std::panic::catch_unwind(|| {
            let _ = parse(&chunks);
        });
        assert!(
            result.is_ok(),
            "parser panicked on case {i}: {:?}",
            chunks
                .iter()
                .map(|(c, _)| String::from_utf8_lossy(&c.text).into_owned())
                .collect::<Vec<_>>()
        );
    }
}
