// Syntax highlighting for VEX in the wrangle editors, and finding the word under
// the text cursor (for F1 help on a function).

use crate::egui::text::{LayoutJob, TextFormat};
use crate::egui::{Color32, FontId};

const KEYWORDS: [&str; 13] = ["if", "else", "for", "foreach", "while", "do", "return", "break", "continue", "const", "export", "function", "void"];
const TYPES: [&str; 7] = ["int", "float", "vector", "string", "vector4", "matrix3", "matrix"];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Plain,
    Comment,
    Str,
    Number,
    Keyword,
    Type,
    Function,
    Attribute,
}

struct Palette {
    plain: Color32,
    comment: Color32,
    string: Color32,
    number: Color32,
    keyword: Color32,
    ty: Color32,
    function: Color32,
    attribute: Color32,
}

fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            plain: Color32::from_gray(220),
            comment: Color32::from_rgb(110, 150, 100),
            string: Color32::from_rgb(214, 157, 133),
            number: Color32::from_rgb(181, 206, 168),
            keyword: Color32::from_rgb(197, 134, 192),
            ty: Color32::from_rgb(78, 201, 176),
            function: Color32::from_rgb(220, 220, 170),
            attribute: Color32::from_rgb(120, 190, 255),
        }
    } else {
        Palette {
            plain: Color32::from_gray(30),
            comment: Color32::from_rgb(0, 128, 0),
            string: Color32::from_rgb(163, 21, 21),
            number: Color32::from_rgb(9, 134, 88),
            keyword: Color32::from_rgb(175, 0, 219),
            ty: Color32::from_rgb(38, 127, 153),
            function: Color32::from_rgb(121, 94, 38),
            attribute: Color32::from_rgb(0, 80, 200),
        }
    }
}

pub fn is_builtin(name: &str) -> bool {
    crate::vex::BUILTINS.iter().any(|b| b.0 == name)
}

fn ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

// The code split into (kind, char range) spans that cover it end to end.
pub fn spans(code: &str) -> Vec<(Kind, std::ops::Range<usize>)> {
    let c: Vec<char> = code.chars().collect();
    let n = c.len();
    let mut out: Vec<(Kind, std::ops::Range<usize>)> = Vec::new();
    let mut push = |kind: Kind, range: std::ops::Range<usize>| {
        if range.is_empty() {
            return;
        }
        match out.last_mut() {
            Some((k, r)) if *k == kind && r.end == range.start => r.end = range.end,
            _ => out.push((kind, range)),
        }
    };
    let mut i = 0;
    while i < n {
        let start = i;
        let ch = c[i];
        // Comments.
        if ch == '/' && c.get(i + 1) == Some(&'/') {
            while i < n && c[i] != '\n' {
                i += 1;
            }
            push(Kind::Comment, start..i);
            continue;
        }
        if ch == '/' && c.get(i + 1) == Some(&'*') {
            i += 2;
            while i < n && !(c[i] == '*' && c.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            i = (i + 2).min(n);
            push(Kind::Comment, start..i);
            continue;
        }
        // Strings.
        if ch == '"' || ch == '\'' {
            i += 1;
            while i < n && c[i] != ch && c[i] != '\n' {
                if c[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(n);
            push(Kind::Str, start..i);
            continue;
        }
        // Attributes: @P, f@mass, v[]@list, i@id, and $F-style variables.
        let typed_attr = ch.is_ascii_alphabetic() && {
            let mut j = i + 1;
            if c.get(j) == Some(&'[') && c.get(j + 1) == Some(&']') {
                j += 2;
            }
            c.get(j) == Some(&'@') && (i == 0 || !ident_char(c[i - 1])) && "fivpsu234".contains(ch)
        };
        if ch == '@' || ch == '$' || typed_attr {
            while i < n && c[i] != '@' && c[i] != '$' {
                i += 1;
            }
            i += 1;
            while i < n && ident_char(c[i]) {
                i += 1;
            }
            push(Kind::Attribute, start..i);
            continue;
        }
        // Numbers, including .5 and 1e-3.
        if ch.is_ascii_digit() || (ch == '.' && c.get(i + 1).is_some_and(|d| d.is_ascii_digit()) && (i == 0 || !ident_char(c[i - 1]))) {
            i += 1;
            while i < n && (c[i].is_ascii_alphanumeric() || c[i] == '.' || ((c[i] == '-' || c[i] == '+') && matches!(c[i - 1], 'e' | 'E'))) {
                i += 1;
            }
            push(Kind::Number, start..i);
            continue;
        }
        // Words.
        if ch.is_ascii_alphabetic() || ch == '_' {
            while i < n && ident_char(c[i]) {
                i += 1;
            }
            let word: String = c[start..i].iter().collect();
            let mut j = i;
            while j < n && c[j] == ' ' {
                j += 1;
            }
            let called = c.get(j) == Some(&'(');
            let kind = if KEYWORDS.contains(&word.as_str()) {
                Kind::Keyword
            } else if TYPES.contains(&word.as_str()) {
                // vector(...) and the like are casts: still types.
                Kind::Type
            } else if called || is_builtin(&word) {
                Kind::Function
            } else {
                Kind::Plain
            };
            push(kind, start..i);
            continue;
        }
        i += 1;
        push(Kind::Plain, start..i);
    }
    out
}

// The highlighted layout of `code`, for a TextEdit's layouter.
pub fn highlight(code: &str, font: FontId, dark: bool) -> LayoutJob {
    let p = palette(dark);
    let mut job = LayoutJob::default();
    let byte_of: Vec<usize> = code.char_indices().map(|(b, _)| b).chain(std::iter::once(code.len())).collect();
    for (kind, range) in spans(code) {
        let colour = match kind {
            Kind::Plain => p.plain,
            Kind::Comment => p.comment,
            Kind::Str => p.string,
            Kind::Number => p.number,
            Kind::Keyword => p.keyword,
            Kind::Type => p.ty,
            Kind::Function => p.function,
            Kind::Attribute => p.attribute,
        };
        let text = &code[byte_of[range.start]..byte_of[range.end]];
        job.append(text, 0.0, TextFormat { font_id: font.clone(), color: colour, italics: kind == Kind::Comment, ..Default::default() });
    }
    job
}

// The identifier the cursor is in or just after (char index), unless it's part of
// an attribute, a string, a comment or a number.
pub fn word_at(code: &str, cursor: usize) -> Option<String> {
    let c: Vec<char> = code.chars().collect();
    let cursor = cursor.min(c.len());
    let at = if cursor < c.len() && ident_char(c[cursor]) {
        cursor
    } else if cursor > 0 && ident_char(c[cursor - 1]) {
        cursor - 1
    } else {
        return None;
    };
    let (kind, range) = spans(code).into_iter().find(|(_, r)| r.contains(&at))?;
    if !matches!(kind, Kind::Function | Kind::Keyword | Kind::Type | Kind::Plain) {
        return None;
    }
    let mut s = at;
    while s > range.start && ident_char(c[s - 1]) {
        s -= 1;
    }
    let mut e = at;
    while e < range.end && ident_char(c[e]) {
        e += 1;
    }
    Some(c[s..e].iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(code: &str) -> Vec<(Kind, String)> {
        let c: Vec<char> = code.chars().collect();
        spans(code).into_iter().filter(|(k, _)| *k != Kind::Plain).map(|(k, r)| (k, c[r].iter().collect())).collect()
    }

    #[test]
    fn colours_each_kind_of_token() {
        let code = "// lift\nfloat h = fit(@P.y, 0, 1.5e-1, 0, 1); v@vel = set(0,1,0); if (h > .5) s@name = \"top\"; /* done */";
        let k = kinds(code);
        assert!(k.contains(&(Kind::Comment, "// lift".into())));
        assert!(k.contains(&(Kind::Type, "float".into())));
        assert!(k.contains(&(Kind::Function, "fit".into())));
        assert!(k.contains(&(Kind::Attribute, "@P".into())));
        assert!(k.contains(&(Kind::Number, "1.5e-1".into())));
        assert!(k.contains(&(Kind::Attribute, "v@vel".into())));
        assert!(k.contains(&(Kind::Keyword, "if".into())));
        assert!(k.contains(&(Kind::Number, ".5".into())));
        assert!(k.contains(&(Kind::Attribute, "s@name".into())));
        assert!(k.contains(&(Kind::Str, "\"top\"".into())));
        assert!(k.contains(&(Kind::Comment, "/* done */".into())));
        // A variable named like a type prefix isn't an attribute.
        assert!(!kinds("f = 1;").iter().any(|(k, _)| *k == Kind::Attribute));
        // Every character is covered, in order.
        let all = spans(code);
        assert_eq!(all.first().unwrap().1.start, 0);
        assert_eq!(all.last().unwrap().1.end, code.chars().count());
        assert!(all.windows(2).all(|w| w[0].1.end == w[1].1.start));
        // The layout keeps the text.
        assert_eq!(highlight(code, FontId::monospace(12.0), true).text, code);
    }

    #[test]
    fn finds_the_word_under_the_cursor() {
        let code = "@P.y += noise(@P * 2); // fit";
        assert_eq!(word_at(code, 10).as_deref(), Some("noise"));
        // Right after the word counts too.
        assert_eq!(word_at(code, 13).as_deref(), Some("noise"));
        assert_eq!(word_at(code, 2), None, "the attribute's component");
        assert_eq!(word_at(code, 1), None, "an attribute");
        assert_eq!(word_at(code, 27), None, "a comment");
        assert_eq!(word_at("x = \"fit\";", 6), None, "a string");
        assert_eq!(word_at("int n;", 1).as_deref(), Some("int"));
        assert_eq!(word_at("", 0), None);
        assert!(is_builtin("noise") && !is_builtin("banana"));
    }
}
