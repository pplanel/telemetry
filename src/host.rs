//! Short host name for the `host.name` resource attribute.

/// `host.name` from `OTEL_RESOURCE_ATTRIBUTES`, else the hostname up to the first dot.
pub(crate) fn host_name() -> String {
    let attrs = std::env::var("OTEL_RESOURCE_ATTRIBUTES").ok();
    from_attributes(attrs.as_deref())
        .unwrap_or_else(|| short(&gethostname::gethostname().to_string_lossy()))
}

fn from_attributes(attrs: Option<&str>) -> Option<String> {
    attrs?.split(',').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        let value = value.trim();
        (key.trim() == "host.name" && !value.is_empty()).then(|| value.to_owned())
    })
}

fn short(hostname: &str) -> String {
    hostname.split('.').next().unwrap_or(hostname).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attribute_wins() {
        let attrs = "deployment.environment=prod, host.name=caladan";
        assert_eq!(from_attributes(Some(attrs)).as_deref(), Some("caladan"));
    }

    #[test]
    fn missing_or_empty_attribute_falls_through() {
        assert_eq!(from_attributes(None), None);
        assert_eq!(from_attributes(Some("a=b")), None);
        assert_eq!(from_attributes(Some("host.name=")), None);
    }

    #[test]
    fn hostname_is_cut_at_first_dot() {
        assert_eq!(short("imperium.local"), "imperium");
        assert_eq!(short("arrakis"), "arrakis");
    }
}
