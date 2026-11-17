// SPDX-License-Identifier: MPL-2.0
use serde::{Deserialize, Serialize};
use std::{fmt, net::IpAddr};

const MAX_BASE_URL_BYTES: usize = 2_048;
const MAX_MODEL_BYTES: usize = 200;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiProvider {
    #[default]
    #[serde(rename = "openai")]
    OpenAi,
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible,
    Anthropic,
    Gemini,
}

impl ApiProvider {
    pub fn label(self) -> &'static str {
        match self {
            Self::OpenAi => "OpenAI",
            Self::OpenAiCompatible => "OpenAI-compatible",
            Self::Anthropic => "Anthropic",
            Self::Gemini => "Gemini",
        }
    }

    pub fn default_base_url(self) -> &'static str {
        match self {
            Self::OpenAi => "https://api.openai.com/v1",
            Self::OpenAiCompatible => "",
            Self::Anthropic => "https://api.anthropic.com/v1",
            Self::Gemini => "https://generativelanguage.googleapis.com/v1beta",
        }
    }

    fn fixed_address(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::OpenAi => Some(("api.openai.com", "/v1")),
            Self::OpenAiCompatible => None,
            Self::Anthropic => Some(("api.anthropic.com", "/v1")),
            Self::Gemini => Some(("generativelanguage.googleapis.com", "/v1beta")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiEndpoint {
    /// A lowercase DNS name or normalized IP address, without IPv6 brackets.
    pub host: String,
    pub port: u16,
    /// Empty for an origin-only base; otherwise starts with '/' and has no final '/'.
    pub base_path: String,
    pub canonical_url: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndpointError {
    InvalidUrl,
    ProviderAddressMismatch,
    InvalidModel,
}

impl fmt::Display for EndpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidUrl => "invalid API base URL",
            Self::ProviderAddressMismatch => "API base URL does not match the selected provider",
            Self::InvalidModel => "invalid model identifier",
        })
    }
}

impl std::error::Error for EndpointError {}

/// Resolve a user-entered HTTPS base URL. Official provider modes never accept
/// alternate hosts, ports, or API prefixes; those require compatible mode.
pub fn resolve(provider: ApiProvider, base_url: &str) -> Result<ApiEndpoint, EndpointError> {
    // Check controls before trimming, so pasted CR/LF is not silently accepted.
    if base_url.len() > MAX_BASE_URL_BYTES || base_url.chars().any(char::is_control) {
        return Err(EndpointError::InvalidUrl);
    }
    let supplied = base_url.trim();
    let url = if supplied.is_empty() {
        provider.default_base_url()
    } else {
        supplied
    };
    let endpoint = parse_https_base(url)?;
    endpoint.check_provider_address(provider)?;
    Ok(endpoint)
}

impl ApiEndpoint {
    pub fn request_path(
        &self,
        provider: ApiProvider,
        model: &str,
    ) -> Result<String, EndpointError> {
        self.check_provider_address(provider)?;
        validate_model_value(model)?;
        let suffix = match provider {
            ApiProvider::OpenAi => "/responses",
            ApiProvider::OpenAiCompatible => "/chat/completions",
            ApiProvider::Anthropic => "/messages",
            ApiProvider::Gemini => {
                let slug = model.strip_prefix("models/").unwrap_or(model);
                if !slug
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                    || !slug
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
                {
                    return Err(EndpointError::InvalidModel);
                }
                return Ok(format!("{}/models/{slug}:generateContent", self.base_path));
            }
        };
        Ok(format!("{}{suffix}", self.base_path))
    }

    fn check_provider_address(&self, provider: ApiProvider) -> Result<(), EndpointError> {
        if let Some((host, path)) = provider.fixed_address() {
            if self.host != host || self.port != 443 || self.base_path != path {
                return Err(EndpointError::ProviderAddressMismatch);
            }
        }
        Ok(())
    }
}

pub(crate) fn validate_model_value(model: &str) -> Result<(), EndpointError> {
    // These models are JSON values, so organization/model and provider tags
    // are valid. Only Gemini inserts a model into the URL and needs a slug.
    if model.is_empty()
        || model.len() > MAX_MODEL_BYTES
        || model.contains("://")
        || !model
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-/@+~".contains(&byte))
    {
        return Err(EndpointError::InvalidModel);
    }
    Ok(())
}

fn parse_https_base(url: &str) -> Result<ApiEndpoint, EndpointError> {
    if !url.is_ascii()
        || url.bytes().any(|byte| {
            byte.is_ascii_whitespace() || byte.is_ascii_control() || b"\\?#".contains(&byte)
        })
        || !url
            .get(..8)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"))
    {
        return Err(EndpointError::InvalidUrl);
    }
    let rest = &url[8..];
    let (authority, path) = rest
        .find('/')
        .map_or((rest, ""), |index| (&rest[..index], &rest[index..]));
    let (host, port, ipv6) = parse_authority(authority)?;
    let base_path = normalize_base_path(path)?;
    let authority = if ipv6 {
        format!("[{host}]")
    } else {
        host.clone()
    };
    let port_suffix = if port == 443 {
        String::new()
    } else {
        format!(":{port}")
    };
    let canonical_url = format!("https://{authority}{port_suffix}{base_path}");
    Ok(ApiEndpoint {
        host,
        port,
        base_path,
        canonical_url,
    })
}

fn parse_authority(authority: &str) -> Result<(String, u16, bool), EndpointError> {
    if authority.is_empty() || authority.contains('@') {
        return Err(EndpointError::InvalidUrl);
    }
    if let Some(bracketed) = authority.strip_prefix('[') {
        let closing = bracketed.find(']').ok_or(EndpointError::InvalidUrl)?;
        let address = bracketed[..closing]
            .parse::<std::net::Ipv6Addr>()
            .map_err(|_| EndpointError::InvalidUrl)?;
        let after = &bracketed[closing + 1..];
        let port = if after.is_empty() {
            443
        } else {
            parse_port(after.strip_prefix(':').ok_or(EndpointError::InvalidUrl)?)?
        };
        return Ok((address.to_string(), port, true));
    }
    if authority.contains(['[', ']']) {
        return Err(EndpointError::InvalidUrl);
    }
    let (host, port) = match authority.split_once(':') {
        Some((host, port)) => (host, parse_port(port)?),
        None => (authority, 443),
    };
    let host = normalize_dns_or_ipv4(host)?;
    Ok((host, port, false))
}

fn parse_port(port: &str) -> Result<u16, EndpointError> {
    if port.is_empty() || port.len() > 5 || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(EndpointError::InvalidUrl);
    }
    port.parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or(EndpointError::InvalidUrl)
}

fn normalize_dns_or_ipv4(host: &str) -> Result<String, EndpointError> {
    if let Ok(IpAddr::V4(address)) = host.parse::<IpAddr>() {
        return Ok(address.to_string());
    }
    if host.is_empty() || host.len() > 253 {
        return Err(EndpointError::InvalidUrl);
    }
    let host = host.to_ascii_lowercase();
    // Reject malformed or legacy numeric IP spellings instead of treating
    // them as DNS names and leaving interpretation to the Windows resolver.
    if host.split('.').all(|part| {
        part.bytes().all(|byte| byte.is_ascii_digit())
            || part.strip_prefix("0x").is_some_and(|hex| {
                !hex.is_empty() && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
    }) {
        return Err(EndpointError::InvalidUrl);
    }
    for label in host.split('.') {
        let bytes = label.as_bytes();
        if bytes.is_empty()
            || bytes.len() > 63
            || !bytes[0].is_ascii_alphanumeric()
            || !bytes[bytes.len() - 1].is_ascii_alphanumeric()
            || !bytes
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
        {
            return Err(EndpointError::InvalidUrl);
        }
    }
    Ok(host)
}

fn normalize_base_path(path: &str) -> Result<String, EndpointError> {
    // No percent decoding: rejecting escapes also rejects encoded traversal,
    // slashes, controls, and different interpretations across HTTP layers.
    if !path
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"/._-~".contains(&byte))
    {
        return Err(EndpointError::InvalidUrl);
    }
    let path = path.trim_end_matches('/');
    if !path.is_empty()
        && path[1..]
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(EndpointError::InvalidUrl);
    }
    if ["/responses", "/chat/completions", "/messages"]
        .iter()
        .any(|suffix| path.ends_with(suffix))
    {
        return Err(EndpointError::InvalidUrl);
    }
    Ok(path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compatible(url: &str) -> Result<ApiEndpoint, EndpointError> {
        resolve(ApiProvider::OpenAiCompatible, url)
    }

    #[test]
    fn provider_storage_tags_and_default_are_stable() {
        assert_eq!(ApiProvider::default(), ApiProvider::OpenAi);
        for (provider, tag) in [
            (ApiProvider::OpenAi, "openai"),
            (ApiProvider::OpenAiCompatible, "openai_compatible"),
            (ApiProvider::Anthropic, "anthropic"),
            (ApiProvider::Gemini, "gemini"),
        ] {
            let serialized = serde_json::to_string(&provider).unwrap();
            assert_eq!(serialized, format!("\"{tag}\""));
            assert_eq!(
                serde_json::from_str::<ApiProvider>(&serialized).unwrap(),
                provider
            );
            assert!(!provider.label().is_empty());
        }
    }

    #[test]
    fn official_modes_resolve_empty_addresses_to_their_defaults() {
        for provider in [
            ApiProvider::OpenAi,
            ApiProvider::Anthropic,
            ApiProvider::Gemini,
        ] {
            for input in ["", "   "] {
                let endpoint = resolve(provider, input).unwrap();
                assert_eq!(endpoint.canonical_url, provider.default_base_url());
                assert_eq!(endpoint.port, 443);
            }
        }
        assert_eq!(compatible(""), Err(EndpointError::InvalidUrl));
        assert_eq!(compatible("   "), Err(EndpointError::InvalidUrl));
    }

    #[test]
    fn canonicalization_normalizes_case_default_port_and_final_slashes() {
        let endpoint = compatible(" HTTPS://API.EXAMPLE.COM:443/openai/v1/// ").unwrap();
        assert_eq!(endpoint.host, "api.example.com");
        assert_eq!(endpoint.port, 443);
        assert_eq!(endpoint.base_path, "/openai/v1");
        assert_eq!(endpoint.canonical_url, "https://api.example.com/openai/v1");
        assert_eq!(compatible(&endpoint.canonical_url).unwrap(), endpoint);
    }

    #[test]
    fn explicit_nondefault_ports_and_origin_only_bases_are_preserved() {
        let endpoint = compatible("https://localhost:8443///").unwrap();
        assert_eq!(endpoint.host, "localhost");
        assert_eq!(endpoint.port, 8443);
        assert_eq!(endpoint.base_path, "");
        assert_eq!(endpoint.canonical_url, "https://localhost:8443");
        assert_eq!(
            endpoint
                .request_path(ApiProvider::OpenAiCompatible, "org/model:free")
                .unwrap(),
            "/chat/completions"
        );
    }

    #[test]
    fn ip_literals_have_unambiguous_connection_hosts_and_canonical_urls() {
        let v4 = compatible("https://192.0.2.10:65535/v1/").unwrap();
        assert_eq!(v4.host, "192.0.2.10");
        assert_eq!(v4.port, 65535);
        let v6 = compatible("https://[2001:0DB8:0:0:0:0:0:1]:8443/v1").unwrap();
        assert_eq!(v6.host, "2001:db8::1");
        assert_eq!(v6.canonical_url, "https://[2001:db8::1]:8443/v1");
        assert_eq!(
            compatible("https://[::1]:443/").unwrap().canonical_url,
            "https://[::1]"
        );
    }

    #[test]
    fn credentials_queries_fragments_controls_and_non_https_inputs_are_rejected() {
        for input in [
            "http://api.example.com/v1",
            "ftp://api.example.com/v1",
            "//api.example.com/v1",
            "/v1",
            "api.example.com/v1",
            "https:///v1",
            "https://",
            "https://user:secret@api.example.com/v1",
            "https://user@api.example.com/v1",
            "https://api.example.com/v1?key=secret",
            "https://api.example.com/v1#fragment",
            "https://api.example.com\\v1",
            "https:\\api.example.com/v1",
            "https://api.example.com/v1\n",
            "\rhttps://api.example.com/v1",
            "\thttps://api.example.com/v1",
            "https://api.example.com/\0v1",
            "https://api.example.com/\u{7f}v1",
            "https://api.example.com/a b",
            "https://api.예시.com/v1",
            "https://api.example.com/경로",
        ] {
            assert_eq!(
                compatible(input),
                Err(EndpointError::InvalidUrl),
                "{input:?}"
            );
        }
    }

    #[test]
    fn invalid_ports_and_brackets_are_rejected() {
        for input in [
            "https://api.example.com:",
            "https://api.example.com:0",
            "https://api.example.com:65536",
            "https://api.example.com:-1",
            "https://api.example.com:+443",
            "https://api.example.com:abc",
            "https://api.example.com:443:80",
            "https://[::1",
            "https://[::1]x",
            "https://[::1]:",
            "https://[::1]:0",
            "https://[::1]:65536",
            "https://::1/v1",
            "https://[127.0.0.1]/v1",
            "https://[fe80::1%25eth0]/v1",
            "https://api.example.com]/v1",
        ] {
            assert_eq!(compatible(input), Err(EndpointError::InvalidUrl), "{input}");
        }
    }

    #[test]
    fn invalid_dns_and_ambiguous_numeric_hosts_are_rejected() {
        for host in [
            "-example.com",
            "example-.com",
            "api..example.com",
            ".example.com",
            "example.com.",
            "api_example.com",
            "api%2eexample.com",
            "256.0.0.1",
            "127.1",
            "127.00.0.1",
            "2130706433",
            "0x7f000001",
            "0x7f.0.0.1",
            "...",
        ] {
            assert_eq!(
                compatible(&format!("https://{host}/v1")),
                Err(EndpointError::InvalidUrl),
                "{host}"
            );
        }
        assert!(compatible(&format!("https://{}.example/v1", "a".repeat(64))).is_err());
        assert!(compatible("https://xn--example-9d0b.test/v1").is_ok());
    }

    #[test]
    fn traversal_encoded_paths_and_interior_empty_segments_are_rejected() {
        for path in [
            "/.",
            "/..",
            "/v1/./api",
            "/v1/../api",
            "/v1//api",
            "/v1/%2e%2e/api",
            "/v1/%2Fapi",
            "/v1/%252e%252e",
            "/v1;parameter/api",
            "/v1/..\\api",
        ] {
            assert_eq!(
                compatible(&format!("https://api.example.com{path}")),
                Err(EndpointError::InvalidUrl),
                "{path}"
            );
        }
    }

    #[test]
    fn complete_request_paths_are_not_accepted_as_bases() {
        for path in [
            "/chat/completions",
            "/v1/chat/completions/",
            "/v1/responses",
            "/v1/messages",
        ] {
            assert_eq!(
                compatible(&format!("https://api.example.com{path}")),
                Err(EndpointError::InvalidUrl)
            );
        }
        assert!(compatible("https://api.example.com/responses-v2/v1").is_ok());
    }

    #[test]
    fn official_provider_modes_reject_changed_hosts_ports_and_prefixes() {
        for provider in [
            ApiProvider::OpenAi,
            ApiProvider::Anthropic,
            ApiProvider::Gemini,
        ] {
            let default = provider.default_base_url();
            let expected = resolve(provider, "").unwrap();
            for url in [
                format!("https://other.example{}", expected.base_path),
                format!("https://{}:8443{}", expected.host, expected.base_path),
                format!("https://{}/custom", expected.host),
                format!("{default}/extra"),
            ] {
                assert_eq!(
                    resolve(provider, &url),
                    Err(EndpointError::ProviderAddressMismatch)
                );
                assert!(compatible(&url).is_ok());
            }
            let normalized = format!(
                "HTTPS://{}:443{}/",
                expected.host.to_ascii_uppercase(),
                expected.base_path
            );
            assert_eq!(resolve(provider, &normalized).unwrap(), expected);
        }
    }

    #[test]
    fn each_provider_uses_its_own_request_route() {
        assert_eq!(
            resolve(ApiProvider::OpenAi, "")
                .unwrap()
                .request_path(ApiProvider::OpenAi, "gpt-6-luna")
                .unwrap(),
            "/v1/responses"
        );
        assert_eq!(
            compatible("https://api.example.com/openai/v1")
                .unwrap()
                .request_path(ApiProvider::OpenAiCompatible, "org/model:free")
                .unwrap(),
            "/openai/v1/chat/completions"
        );
        assert_eq!(
            resolve(ApiProvider::Anthropic, "")
                .unwrap()
                .request_path(ApiProvider::Anthropic, "claude-model")
                .unwrap(),
            "/v1/messages"
        );
        assert_eq!(
            resolve(ApiProvider::Gemini, "")
                .unwrap()
                .request_path(ApiProvider::Gemini, "gemini-2.5-flash")
                .unwrap(),
            "/v1beta/models/gemini-2.5-flash:generateContent"
        );
    }

    #[test]
    fn json_model_names_can_contain_organization_paths_and_provider_tags() {
        for model in [
            "org/model",
            "org/model:free",
            "org/model-v2.1@latest",
            "model+tag",
            "model~tag",
        ] {
            assert_eq!(
                compatible("https://api.example.com/v1")
                    .unwrap()
                    .request_path(ApiProvider::OpenAiCompatible, model)
                    .unwrap(),
                "/v1/chat/completions"
            );
        }
    }

    #[test]
    fn gemini_accepts_one_optional_models_prefix_and_rejects_path_insertion() {
        let endpoint = resolve(ApiProvider::Gemini, "").unwrap();
        assert_eq!(
            endpoint
                .request_path(ApiProvider::Gemini, "models/gemini-2.5-flash")
                .unwrap(),
            "/v1beta/models/gemini-2.5-flash:generateContent"
        );
        for model in [
            "models/",
            "models/models/gemini",
            "org/model",
            ".",
            "..",
            "../gemini",
            "/gemini",
            "gemini:generateContent",
            "gemini%2fother",
            "gemini?key=x",
            "gemini#x",
        ] {
            assert_eq!(
                endpoint.request_path(ApiProvider::Gemini, model),
                Err(EndpointError::InvalidModel),
                "{model}"
            );
        }
    }

    #[test]
    fn invalid_model_values_are_rejected_without_echoing_them() {
        let endpoint = compatible("https://api.example.com/v1").unwrap();
        for model in [
            "",
            " ",
            "org/model\n",
            "model\"secret",
            "model\\secret",
            "model?key=secret",
            "https://attacker.example",
        ] {
            let error = endpoint
                .request_path(ApiProvider::OpenAiCompatible, model)
                .unwrap_err();
            assert_eq!(error, EndpointError::InvalidModel);
            assert_eq!(error.to_string(), "invalid model identifier");
        }
        assert!(endpoint
            .request_path(ApiProvider::OpenAiCompatible, &"a".repeat(200))
            .is_ok());
        assert_eq!(
            endpoint.request_path(ApiProvider::OpenAiCompatible, &"a".repeat(201)),
            Err(EndpointError::InvalidModel)
        );
    }

    #[test]
    fn request_paths_cannot_reuse_a_custom_endpoint_as_an_official_provider() {
        let endpoint = compatible("https://api.example.com/v1").unwrap();
        for provider in [
            ApiProvider::OpenAi,
            ApiProvider::Anthropic,
            ApiProvider::Gemini,
        ] {
            assert_eq!(
                endpoint.request_path(provider, "model"),
                Err(EndpointError::ProviderAddressMismatch)
            );
        }
    }

    #[test]
    fn oversized_urls_are_rejected_and_url_errors_do_not_echo_input() {
        assert_eq!(
            compatible(&format!("https://api.example.com/{}", "a".repeat(2_048))),
            Err(EndpointError::InvalidUrl)
        );
        let error = compatible("https://user:private-secret@api.example.com/v1").unwrap_err();
        assert_eq!(error.to_string(), "invalid API base URL");
        assert!(!format!("{error:?}").contains("private-secret"));
    }
}
