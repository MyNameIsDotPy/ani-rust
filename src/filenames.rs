pub fn sanitize(s: &str) -> String {
    let mut result: String = s
        .chars()
        .take(80)
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if result.is_empty() {
        result.push('_');
    }
    format!("ani-{result}")
}
#[cfg(test)]
mod tests {
    #[test]
    fn safe() {
        for s in ["../a:b", "CON", "", "a/b\\c"] {
            let x = super::sanitize(s);
            assert!(!x.contains(['/', '\\', ':', '.']));
            assert!(x.starts_with("ani-"));
        }
    }
}
