pub(crate) fn doc_scope_for<'a>(
    rel: &str,
    default_scope: &'a str,
    config: &crate::config::Config,
) -> std::borrow::Cow<'a, str> {
    if !crate::path::is_external_key(rel.as_bytes()) {
        return std::borrow::Cow::Borrowed(default_scope);
    }
    let mut best: Option<String> = None;
    for raw_root in &config.scan.extra_roots {
        if let Ok(canonical) = raw_root.canonicalize()
            && let Some(prefix) = canonical.to_str()
        {
            #[cfg(windows)]
            let prefix = prefix.replace('\\', "/");
            #[cfg(windows)]
            let prefix = prefix.as_str();
            let prefix = prefix.trim_end_matches('/');
            let inside = rel == prefix || rel.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('/'));
            if inside && best.as_ref().is_none_or(|b| prefix.len() > b.len()) {
                best = Some(prefix.to_string());
            }
        }
    }
    if let Some(prefix) = best {
        return std::borrow::Cow::Owned(format!("path:{prefix}"));
    }
    std::borrow::Cow::Owned(format!("path:{rel}"))
}
