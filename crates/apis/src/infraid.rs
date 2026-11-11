//! `github.com/openshift/hypershift/support/infraid`

/// `utilrand` alphabet (`k8s.io/apimachinery/pkg/util/rand`): no vowels, no confusable chars.
const ALPHANUMS: &[u8] = b"bcdfghjklmnpqrstvwxz2456789";
const RANDOM_LEN: usize = 5;

/// `infraid.New(clusterName)`: sanitized, truncated cluster name plus 5 random characters.
pub fn new(cluster_name: &str) -> String {
    generate_infra_id(cluster_name, 27, &mut || {
        ALPHANUMS[fastrand::usize(..ALPHANUMS.len())] as char
    })
}

/// `generateInfraID` with an injectable random source (for tests).
pub fn generate_infra_id(
    base: &str,
    max_len: usize,
    rand_char: &mut dyn FnMut() -> char,
) -> String {
    let max_base_len = max_len - (RANDOM_LEN + 1);

    // replace all characters that are not `alphanum` or `-` with `-` (per UTF-8 rune, like
    // Go's regexp)
    let replaced: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();

    // replace all multiple dashes in a sequence with single one.
    let mut collapsed = String::with_capacity(replaced.len());
    for c in replaced.chars() {
        if c == '-' && collapsed.ends_with('-') {
            continue;
        }
        collapsed.push(c);
    }

    // truncate to maxBaseLen (bytes; the string is ASCII at this point)
    let mut b = collapsed;
    if b.len() > max_base_len {
        b.truncate(max_base_len);
    }
    let b = b.trim_end_matches('-');

    let suffix: String = (0..RANDOM_LEN).map(|_| rand_char()).collect();
    format!("{b}-{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed() -> impl FnMut() -> char {
        || 'x'
    }

    #[test]
    fn sanitizes_and_truncates() {
        assert_eq!(
            generate_infra_id("my-cluster", 27, &mut fixed()),
            "my-cluster-xxxxx"
        );
        assert_eq!(
            generate_infra_id("my_cluster..x", 27, &mut fixed()),
            "my-cluster-x-xxxxx"
        );
        assert_eq!(
            generate_infra_id("a-very-long-cluster-name-that-exceeds", 27, &mut fixed()),
            "a-very-long-cluster-n-xxxxx"
        );
        assert_eq!(
            generate_infra_id("abcdefghijklmnopqrst-uvwxyz", 27, &mut fixed()),
            "abcdefghijklmnopqrst-xxxxx"
        );
        let id = new("test");
        assert_eq!(id.len(), 10);
        assert!(id.starts_with("test-"));
        assert!(id[5..].bytes().all(|c| ALPHANUMS.contains(&c)));
    }
}
