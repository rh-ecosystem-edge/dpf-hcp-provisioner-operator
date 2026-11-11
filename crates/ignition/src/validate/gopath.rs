//! Go `path` package functions used by the validators.

/// `path.IsAbs`.
pub(crate) fn is_abs(p: &str) -> bool {
    p.starts_with('/')
}

/// `path.Clean`.
pub(crate) fn clean(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let p = path.as_bytes();
    let rooted = p[0] == b'/';
    let n = p.len();
    let mut out: Vec<u8> = Vec::with_capacity(n);
    let (mut r, mut dotdot) = (0usize, 0usize);
    if rooted {
        out.push(b'/');
        r = 1;
        dotdot = 1;
    }
    while r < n {
        if p[r] == b'/' || (p[r] == b'.' && (r + 1 == n || p[r + 1] == b'/')) {
            r += 1;
        } else if p[r] == b'.' && p[r + 1] == b'.' && (r + 2 == n || p[r + 2] == b'/') {
            r += 2;
            if out.len() > dotdot {
                let mut w = out.len() - 1;
                while w > dotdot && out[w] != b'/' {
                    w -= 1;
                }
                out.truncate(w);
            } else if !rooted {
                if !out.is_empty() {
                    out.push(b'/');
                }
                out.extend_from_slice(b"..");
                dotdot = out.len();
            }
        } else {
            if (rooted && out.len() != 1) || (!rooted && !out.is_empty()) {
                out.push(b'/');
            }
            while r < n && p[r] != b'/' {
                out.push(p[r]);
                r += 1;
            }
        }
    }
    if out.is_empty() {
        return ".".to_string();
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `path.Join`.
pub(crate) fn join(elems: &[&str]) -> String {
    let non_empty: Vec<&str> = elems.iter().copied().filter(|e| !e.is_empty()).collect();
    if non_empty.is_empty() {
        return String::new();
    }
    clean(&non_empty.join("/"))
}

/// `path.Ext`.
pub(crate) fn ext(path: &str) -> &str {
    let b = path.as_bytes();
    let mut i = b.len();
    while i > 0 && b[i - 1] != b'/' {
        i -= 1;
        if b[i] == b'.' {
            return &path[i..];
        }
    }
    ""
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_table() {
        for (i, o) in [
            ("", "."),
            ("abc", "abc"),
            ("abc/def", "abc/def"),
            ("a/b/c", "a/b/c"),
            (".", "."),
            ("..", ".."),
            ("../..", "../.."),
            ("../../abc", "../../abc"),
            ("/abc", "/abc"),
            ("/", "/"),
            ("abc/", "abc"),
            ("abc/def/", "abc/def"),
            ("a/b/c/", "a/b/c"),
            ("./", "."),
            ("../", ".."),
            ("../../", "../.."),
            ("/abc/", "/abc"),
            ("abc//def//ghi", "abc/def/ghi"),
            ("//abc", "/abc"),
            ("///abc", "/abc"),
            ("//abc//", "/abc"),
            ("abc//", "abc"),
            ("abc/./def", "abc/def"),
            ("/./abc/def", "/abc/def"),
            ("abc/.", "abc"),
            ("abc/def/ghi/../jkl", "abc/def/jkl"),
            ("abc/def/../ghi/../jkl", "abc/jkl"),
            ("abc/def/..", "abc"),
            ("abc/def/../..", "."),
            ("/abc/def/../..", "/"),
            ("abc/def/../../..", ".."),
            ("/abc/def/../../..", "/"),
            ("abc/def/../../../ghi/jkl/../../../mno", "../../mno"),
            ("abc/./../def", "def"),
            ("abc//./../def", "def"),
            ("abc/../../././../def", "../../def"),
        ] {
            assert_eq!(clean(i), o, "clean({i:?})");
        }
    }

    #[test]
    fn ext_table() {
        assert_eq!(ext("path.go"), ".go");
        assert_eq!(ext("path.pb.go"), ".go");
        assert_eq!(ext("a.dir/b"), "");
        assert_eq!(ext("a.dir/b.go"), ".go");
        assert_eq!(ext("a.dir/"), "");
        assert_eq!(ext(".service"), ".service");
        assert_eq!(ext(""), "");
    }
}
