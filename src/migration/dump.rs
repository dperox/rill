use super::Problem;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

#[derive(Clone)]
pub struct Directive {
    pub file: String,
    pub line: usize,
    pub words: Vec<String>,
    pub block: bool,
    pub children: Vec<Directive>,
}

fn parse(file: &str, source: &str) -> Result<Vec<Directive>, Problem> {
    let mut tree = Vec::new();
    let mut stack = Vec::new();
    let mut words = Vec::new();
    let mut word = String::new();
    let (mut line, mut start) = (1, 1);
    let mut quote = None;
    let mut active = false;
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        if !active && words.is_empty() {
            start = line;
        }
        if c == '\n' {
            line += 1;
        }
        if c == '\\' {
            // Keep escapes literal so a value requiring NGINX-specific decoding is flagged.
            active = true;
            word.push(c);
            let next = chars.next().ok_or_else(|| Problem::syntax(file, line))?;
            if next == '\n' {
                line += 1;
            }
            word.push(next);
        } else if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                word.push(c);
            }
        } else if c == '\'' || c == '"' {
            active = true;
            quote = Some(c);
        } else if c == '#' && !active {
            while chars.next_if(|&v| v != '\n').is_some() {}
        } else if c == '{' && word.ends_with('$') {
            word.push(c);
            loop {
                let next = chars.next().ok_or_else(|| Problem::syntax(file, line))?;
                if next.is_whitespace() {
                    return Err(Problem::syntax(file, line));
                }
                word.push(next);
                if next == '}' {
                    break;
                }
            }
        } else if c.is_whitespace() || matches!(c, ';' | '{' | '}') {
            if active {
                words.push(std::mem::take(&mut word));
                active = false;
            }
            if c.is_whitespace() {
                continue;
            }
            if c == '}' {
                if !words.is_empty() {
                    return Err(Problem::syntax(file, line));
                }
                let (parent, mut node): (_, Directive) =
                    stack.pop().ok_or_else(|| Problem::syntax(file, line))?;
                node.children = std::mem::replace(&mut tree, parent);
                tree.push(node);
            } else {
                if words.is_empty() {
                    return Err(Problem::syntax(file, line));
                }
                let node = Directive {
                    file: file.into(),
                    line: start,
                    words: std::mem::take(&mut words),
                    block: c == '{',
                    children: Vec::new(),
                };
                if node.block {
                    if stack.len() >= 64 {
                        return Err(Problem::syntax(file, line));
                    }
                    stack.push((std::mem::take(&mut tree), node));
                } else {
                    tree.push(node);
                }
            }
        } else {
            active = true;
            word.push(c);
        }
    }
    if active || !words.is_empty() || quote.is_some() || !stack.is_empty() {
        return Err(Problem::syntax(file, line));
    }
    Ok(tree)
}

pub struct Dump {
    pub root: String,
    pub files: BTreeMap<String, Vec<Directive>>,
}

impl Dump {
    pub fn read(text: &str, source: &str) -> Result<Self, Problem> {
        let mut sections = Vec::new();
        let mut name = source.to_string();
        let mut marked = false;
        let mut body = String::new();
        for line in text.lines() {
            if let Some(path) = line
                .strip_prefix("# configuration file ")
                .and_then(|v| v.strip_suffix(':'))
            {
                if marked || !body.trim().is_empty() {
                    sections.push((name, std::mem::take(&mut body)));
                }
                body.clear();
                marked = true;
                name = path.to_owned();
            } else {
                body.push_str(line);
                body.push('\n');
            }
        }
        sections.push((name, body));
        let root = sections[0].0.clone();
        let mut files = BTreeMap::new();
        for (file, src) in sections {
            let nodes = parse(&file, &src)?;
            if files.insert(file.clone(), nodes).is_some() {
                return Err(Problem::syntax(&file, 1));
            }
        }
        Ok(Self { root, files })
    }

    // Match exact paths or one '*' in the filename, using only sections present in the dump.
    pub fn included(&self, pattern: &str) -> Vec<&String> {
        let pattern = Path::new(&self.root)
            .parent()
            .unwrap_or(Path::new(""))
            .join(pattern);
        let mut found = BTreeSet::new();
        for file in self.files.keys() {
            let path = Path::new(file);
            if path == pattern {
                found.insert(file);
                continue;
            }
            if path.parent() != pattern.parent() {
                continue;
            }
            if let Some((a, b)) = pattern
                .file_name()
                .and_then(|v| v.to_str())
                .and_then(|v| v.split_once('*'))
                && let Some(n) = path.file_name().and_then(|v| v.to_str())
                && n.len() >= a.len() + b.len()
                && n.starts_with(a)
                && n.ends_with(b)
            {
                found.insert(file);
            }
        }
        found.into_iter().collect()
    }
}
