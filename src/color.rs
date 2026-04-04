use std::env;
use std::fmt;
use std::io::{IsTerminal, stderr, stdout};

pub struct Painted<T> {
    open: &'static str,
    close: &'static str,
    content: T,
}

impl<T: fmt::Display> fmt::Display for Painted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}{}", self.open, self.content, self.close)
    }
}

pub struct Painter {
    red: &'static str,
    green: &'static str,
    red_bold: &'static str,
    yellow_bold: &'static str,
    reset: &'static str,
}

impl Painter {
    const fn empty() -> Self {
        Self {
            red: "",
            green: "",
            red_bold: "",
            yellow_bold: "",
            reset: "",
        }
    }

    const fn ansi() -> Self {
        Self {
            red: "\x1b[31m",
            green: "\x1b[32m",
            red_bold: "\x1b[31;1m",
            yellow_bold: "\x1b[33;1m",
            reset: "\x1b[0m",
        }
    }

    pub fn good<T: fmt::Display>(&self, content: T) -> Painted<T> {
        Painted {
            open: self.green,
            close: self.reset,
            content,
        }
    }

    pub fn bad<T: fmt::Display>(&self, content: T) -> Painted<T> {
        Painted {
            open: self.red,
            close: self.reset,
            content,
        }
    }

    pub fn emphasis<T: fmt::Display>(&self, content: T) -> Painted<T> {
        Painted {
            open: self.yellow_bold,
            close: self.reset,
            content,
        }
    }
}

fn use_color(is_tty: bool, no_color: bool, term_is_dumb: bool) -> bool {
    is_tty && !no_color && !term_is_dumb
}

fn from_env(is_tty: bool) -> Painter {
    let no_color = env::var_os("NO_COLOR").is_some();
    let term_is_dumb = env::var("TERM").as_deref() == Ok("dumb");
    if use_color(is_tty, no_color, term_is_dumb) {
        Painter::ansi()
    } else {
        Painter::empty()
    }
}

pub fn stdout_painter() -> Painter {
    from_env(stdout().is_terminal())
}

pub fn stderr_painter() -> Painter {
    from_env(stderr().is_terminal())
}

pub fn print_error(message: impl fmt::Display) {
    let p = stderr_painter();
    eprintln!("{}sure-rm:{} {message}", p.red_bold, p.reset);
}

pub fn print_warning(message: impl fmt::Display) {
    let p = stderr_painter();
    eprintln!("{}sure-rm:{} {message}", p.yellow_bold, p.reset);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tty_without_overrides_is_colored() {
        assert!(use_color(true, false, false));
    }

    #[test]
    fn non_tty_is_never_colored() {
        assert!(!use_color(false, false, false));
    }

    #[test]
    fn no_color_disables_ansi() {
        assert!(!use_color(true, true, false));
    }

    #[test]
    fn dumb_term_disables_ansi() {
        assert!(!use_color(true, false, true));
    }
}
