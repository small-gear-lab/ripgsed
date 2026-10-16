//! GREP_COLORS: the colors of the parts of an output line.

/// The Select Graphic Rendition strings of the parts of a line, as `GREP_COLORS` sets them.
#[derive(Debug, Clone)]
pub struct Colors {
    pub selected_match: String,
    pub context_match: String,
    pub filename: String,
    pub line_number: String,
    pub byte_offset: String,
    pub separator: String,
    pub selected_line: String,
    pub context_line: String,
    /// `rv`: `sl` and `cx` swap when `-v` is given.
    pub reverse: bool,
    /// `ne`: no `\33[K` after the sequences.
    pub no_erase: bool,
}

impl Default for Colors {
    fn default() -> Self {
        Colors {
            selected_match: "01;31".into(),
            context_match: "01;31".into(),
            filename: "35".into(),
            line_number: "32".into(),
            byte_offset: "32".into(),
            separator: "36".into(),
            selected_line: String::new(),
            context_line: String::new(),
            reverse: false,
            no_erase: false,
        }
    }
}

impl Colors {
    /// The colors of the environment: `GREP_COLORS`, and `GREP_COLOR` (obsolete) for the match.
    pub fn from_env(program: &str) -> Colors {
        let mut colors = Colors::default();
        if let Ok(value) = std::env::var("GREP_COLOR") {
            if !value.is_empty() {
                eprintln!("{program}: warning: GREP_COLOR='{value}' is deprecated; use GREP_COLORS='mt={value}'");
                colors.selected_match = value.clone();
                colors.context_match = value;
            }
        }
        if let Ok(value) = std::env::var("GREP_COLORS") {
            colors.apply(&value);
        }
        colors
    }

    /// Reads `name=value:name=value:...`; reading stops at the first thing it does not know.
    pub fn apply(&mut self, spec: &str) {
        for item in spec.split(':') {
            let (name, value) = match item.split_once('=') {
                Some((n, v)) => (n, Some(v)),
                None => (item, None),
            };
            let valid = |v: &str| v.bytes().all(|b| b.is_ascii_digit() || b == b';');
            match (name, value) {
                ("mt", Some(v)) if valid(v) => {
                    self.selected_match = v.into();
                    self.context_match = v.into();
                }
                ("ms", Some(v)) if valid(v) => self.selected_match = v.into(),
                ("mc", Some(v)) if valid(v) => self.context_match = v.into(),
                ("sl", Some(v)) if valid(v) => self.selected_line = v.into(),
                ("cx", Some(v)) if valid(v) => self.context_line = v.into(),
                ("fn", Some(v)) if valid(v) => self.filename = v.into(),
                ("ln", Some(v)) if valid(v) => self.line_number = v.into(),
                ("bn", Some(v)) if valid(v) => self.byte_offset = v.into(),
                ("se", Some(v)) if valid(v) => self.separator = v.into(),
                ("rv", None) => self.reverse = true,
                ("ne", None) => self.no_erase = true,
                // A name that is not known is skipped; a known one with a bad value ends the list.
                ("mt" | "ms" | "mc" | "sl" | "cx" | "fn" | "ln" | "bn" | "se" | "rv" | "ne", _) => return,
                _ => {}
            }
        }
    }
}
