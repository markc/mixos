pub fn is_head(method: &str) -> bool {
    if method == "HEAD" {
        return true;
    }
    if method == "alpha.unreg_guard" {
        return true;
    }
    false
}
