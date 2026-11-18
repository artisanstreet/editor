use super::{MAX_SHELL_DEPTH, Shell, program_name};

const RUNS_ITS_ARGUMENTS: &[&str] = &["if", "for", "do", "else", "call", "start"];

const COMMAND_DELIMITERS: &[char] = &[';', ',', '='];

const INTERNAL_COMMAND_ENDS: &[char] = &['/', '+', '[', ']', '.', ':', '\\'];

pub(super) fn parse(text: &str, depth: usize, out: &mut Shell) {
    if depth > MAX_SHELL_DEPTH {
        out.complete = false;
        return;
    }
    if text.contains(['%', '\r', '\n']) {
        out.complete = false;
    }
    let mut lexer = Lexer {
        chars: text.chars().collect(),
        at: 0,
        depth,
        out,
        words: Vec::new(),
        word: String::new(),
        started: false,
        target: false,
        seen: Seen::default(),
    };
    lexer.run();
    if lexer.seen.pipe && lexer.seen.caret {
        lexer.out.complete = false;
        let unescaped: String = text.chars().filter(|c| *c != '^').collect();
        let mut again = Shell {
            commands: Vec::new(),
            aliases: Vec::new(),
            complete: true,
        };
        parse(&unescaped, depth + 1, &mut again);
        lexer.out.aliases.extend(again.commands);
        lexer.out.aliases.extend(again.aliases);
    }
}

struct Lexer<'a> {
    chars: Vec<char>,
    at: usize,
    depth: usize,
    out: &'a mut Shell,
    words: Vec<String>,
    word: String,
    started: bool,
    target: bool,
    seen: Seen,
}

#[derive(Default)]
struct Seen {
    pipe: bool,
    caret: bool,
}

impl Lexer<'_> {
    fn peek(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.at + ahead).copied()
    }

    fn push(&mut self, c: char) {
        self.word.push(c);
        self.started = true;
    }

    fn run(&mut self) {
        while let Some(c) = self.peek(0) {
            match c {
                '^' => {
                    self.seen.caret = true;
                    match self.peek(1) {
                        Some('\r' | '\n') | None => {
                            self.out.complete = false;
                            self.at += 1;
                        }
                        Some(next) => {
                            if next == '"' {
                                self.out.complete = false;
                            }
                            self.push(next);
                            self.at += 2;
                        }
                    }
                }
                '"' => self.quoted(),
                ' ' | '\t' => {
                    self.end_word();
                    self.at += 1;
                }
                '&' => {
                    self.end_command();
                    self.at += if self.peek(1) == Some('&') { 2 } else { 1 };
                }
                '|' => {
                    self.end_command();
                    if self.peek(1) == Some('|') {
                        self.at += 2;
                    } else {
                        self.seen.pipe = true;
                        self.at += 1;
                    }
                }
                '<' | '>' => self.redirect(),
                '(' | ')' | '\r' | '\n' => {
                    self.out.complete = false;
                    self.end_command();
                    self.at += 1;
                }
                _ => {
                    self.push(c);
                    self.at += 1;
                }
            }
        }
        self.end_command();
    }

    fn quoted(&mut self) {
        if self.at > 0 && self.chars[self.at - 1] == '\\' {
            self.out.complete = false;
        }
        self.at += 1;
        self.started = true;
        loop {
            match self.peek(0) {
                None | Some('\r' | '\n') => {
                    self.out.complete = false;
                    return;
                }
                Some('"') => {
                    if self.chars[self.at - 1] == '\\' || self.peek(1) == Some('"') {
                        self.out.complete = false;
                    }
                    self.at += 1;
                    return;
                }
                Some(c) => {
                    self.push(c);
                    self.at += 1;
                }
            }
        }
    }

    fn redirect(&mut self) {
        let handle = self.started
            && !self.word.is_empty()
            && self.word.chars().all(|c| c.is_ascii_digit())
            && self.at > 0
            && self.chars[self.at - 1].is_ascii_digit();
        if handle {
            self.word.clear();
            self.started = false;
        } else {
            self.end_word();
        }
        let doubled = self.peek(1) == Some(self.chars[self.at]);
        self.at += if doubled { 2 } else { 1 };
        if self.peek(0) == Some('&') && self.peek(1).is_some_and(|c| c.is_ascii_digit()) {
            self.at += 2;
        } else {
            self.target = true;
        }
    }

    fn end_word(&mut self) {
        if self.started {
            let word = std::mem::take(&mut self.word);
            if self.target {
                self.target = false;
            } else {
                self.words.push(word);
            }
        }
        self.word.clear();
        self.started = false;
    }

    fn end_command(&mut self) {
        self.end_word();
        if self.target {
            self.target = false;
            self.out.complete = false;
        }
        let words = std::mem::take(&mut self.words);
        simple(words, self.depth, self.out);
    }
}

fn simple(mut words: Vec<String>, depth: usize, out: &mut Shell) {
    while let Some(first) = words.first_mut() {
        let bare = first.trim_start_matches(['@', ';', ',', '=']);
        if bare.is_empty() {
            words.remove(0);
        } else {
            *first = bare.to_owned();
            break;
        }
    }
    let Some(first) = words.first() else {
        return;
    };
    if let Some(at) = first.find(COMMAND_DELIMITERS) {
        let rest = first[at + 1..]
            .trim_start_matches(COMMAND_DELIMITERS)
            .to_owned();
        let program = first[..at].to_owned();
        words[0] = program;
        if !rest.is_empty() {
            words.insert(1, rest);
        }
    }
    let program = words[0].clone();
    if program.contains(['/', '+', '[', ']']) {
        out.complete = false;
    }
    if let Some(at) = program.find(INTERNAL_COMMAND_ENDS).filter(|at| *at > 0) {
        let mut alias = words.clone();
        program[..at].clone_into(&mut alias[0]);
        out.aliases.push(alias);
    }
    let lowered = program_name(&program).to_ascii_lowercase();
    if lowered != program {
        let mut alias = words.clone();
        alias[0].clone_from(&lowered);
        out.aliases.push(alias);
    }
    if RUNS_ITS_ARGUMENTS.contains(&lowered.as_str()) {
        out.complete = false;
        for start in 1..words.len() {
            out.aliases.push(words[start..].to_vec());
        }
    }
    if lowered == "cmd" {
        out.complete = false;
        if let Some(script) = cmd_script(&words) {
            let mut nested = Shell {
                commands: Vec::new(),
                aliases: Vec::new(),
                complete: true,
            };
            parse(&script, depth + 1, &mut nested);
            out.aliases.extend(nested.commands);
            out.aliases.extend(nested.aliases);
        }
    }
    out.commands.push(words);
}

fn cmd_script(words: &[String]) -> Option<String> {
    let at = words.iter().skip(1).position(|word| {
        let lowered = word.to_ascii_lowercase();
        lowered.starts_with("/c") || lowered.starts_with("/k")
    })? + 1;
    let inline = &words[at][2..];
    let mut script: Vec<&str> = Vec::new();
    if !inline.is_empty() {
        script.push(inline);
    }
    script.extend(words[at + 1..].iter().map(String::as_str));
    Some(script.join(" "))
}

#[cfg(test)]
#[path = "../../../tests/claudeai/rules/cmd.rs"]
mod tests;
