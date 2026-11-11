//! The subset of Go `text/template` used by `dpu-agent.service.tmpl`: text, comments,
//! `{{if .Field}}`/`{{else}}`/`{{end}}` over boolean fields, and the `{{-`/`-}}` trim markers.

#[derive(Debug)]
enum Node {
    Text(String),
    If {
        field: String,
        then: Vec<Node>,
        otherwise: Vec<Node>,
    },
}

/// A parsed template.
#[derive(Debug)]
pub(crate) struct Template {
    nodes: Vec<Node>,
}

enum Tok {
    Text(String),
    Action(String),
}

fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n')
}

fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let mut toks = Vec::new();
    let mut rest = src;
    let mut trim_next_left = false;
    while !rest.is_empty() {
        let Some(open) = rest.find("{{") else {
            let mut t = rest.to_string();
            if trim_next_left {
                t = t.trim_start_matches(is_space).to_string();
            }
            toks.push(Tok::Text(t));
            break;
        };
        let mut text = rest[..open].to_string();
        if trim_next_left {
            text = text.trim_start_matches(is_space).to_string();
        }
        let after = &rest[open + 2..];
        let (trim_left, inner_start) = if after.starts_with("- ")
            || after.starts_with("-\t")
            || after.starts_with("-\n")
            || after.starts_with("-\r")
        {
            (true, 1)
        } else {
            (false, 0)
        };
        if trim_left {
            text = text.trim_end_matches(is_space).to_string();
        }
        if !text.is_empty() {
            toks.push(Tok::Text(text));
        }
        let body = &after[inner_start..];
        let close = body
            .find("}}")
            .ok_or_else(|| "unclosed action".to_string())?;
        let mut inner = &body[..close];
        trim_next_left = false;
        if inner.ends_with(" -")
            || inner.ends_with("\t-")
            || inner.ends_with("\n-")
            || inner.ends_with("\r-")
        {
            inner = &inner[..inner.len() - 1];
            trim_next_left = true;
        }
        toks.push(Tok::Action(inner.trim_matches(is_space).to_string()));
        rest = &body[close + 2..];
    }
    Ok(toks)
}

fn parse_list(
    toks: &mut std::iter::Peekable<std::vec::IntoIter<Tok>>,
    depth: usize,
) -> Result<(Vec<Node>, Option<String>), String> {
    let mut nodes = Vec::new();
    while let Some(t) = toks.next() {
        match t {
            Tok::Text(s) => nodes.push(Node::Text(s)),
            Tok::Action(a) => {
                if a.starts_with("/*") {
                    continue;
                }
                let mut words = a.split_whitespace();
                match words.next() {
                    Some("if") => {
                        let field = words.next().ok_or("missing value for if")?;
                        let field = field
                            .strip_prefix('.')
                            .ok_or_else(|| format!("unsupported if argument {field}"))?;
                        let (then, end) = parse_list(toks, depth + 1)?;
                        let otherwise = if end.as_deref() == Some("else") {
                            let (o, end2) = parse_list(toks, depth + 1)?;
                            if end2.as_deref() != Some("end") {
                                return Err("unexpected EOF".into());
                            }
                            o
                        } else if end.as_deref() == Some("end") {
                            Vec::new()
                        } else {
                            return Err("unexpected EOF".into());
                        };
                        nodes.push(Node::If {
                            field: field.to_string(),
                            then,
                            otherwise,
                        });
                    }
                    Some(kw @ ("else" | "end")) => {
                        if depth == 0 {
                            return Err(format!("unexpected {{{{{kw}}}}}"));
                        }
                        return Ok((nodes, Some(kw.to_string())));
                    }
                    _ => return Err(format!("unsupported action {a:?}")),
                }
            }
        }
    }
    Ok((nodes, None))
}

/// Parses a template.
pub(crate) fn parse(src: &str) -> Result<Template, String> {
    let toks = lex(src)?;
    let mut it = toks.into_iter().peekable();
    let (nodes, _) = parse_list(&mut it, 0)?;
    Ok(Template { nodes })
}

impl Template {
    /// Executes the template; `field` resolves boolean fields of the data.
    pub(crate) fn execute(&self, field: &dyn Fn(&str) -> Option<bool>) -> Result<String, String> {
        let mut out = String::new();
        exec(&self.nodes, field, &mut out)?;
        Ok(out)
    }
}

fn exec(
    nodes: &[Node],
    field: &dyn Fn(&str) -> Option<bool>,
    out: &mut String,
) -> Result<(), String> {
    for n in nodes {
        match n {
            Node::Text(s) => out.push_str(s),
            Node::If {
                field: f,
                then,
                otherwise,
            } => {
                let v = field(f).ok_or_else(|| format!("can't evaluate field {f}"))?;
                exec(if v { then } else { otherwise }, field, out)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn trim_markers() {
        let t = parse("a  {{- if .X }} b {{- end }}\nc").unwrap();
        assert_eq!(t.execute(&|_| Some(true)).unwrap(), "a b\nc");
        assert_eq!(t.execute(&|_| Some(false)).unwrap(), "a\nc");
        let t = parse("x{{if .X -}}  y  {{- else}}z{{end}}").unwrap();
        assert_eq!(t.execute(&|_| Some(true)).unwrap(), "xy");
        assert_eq!(t.execute(&|_| Some(false)).unwrap(), "xz");
    }
}
