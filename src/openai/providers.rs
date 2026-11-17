// SPDX-License-Identifier: MPL-2.0
//! Pure protocol adapters. They never send requests, retry, or expose server text.
use super::{
    ApiError, TranslationRequest, TranslationResponse, MAX_OUTPUT_TOKENS, MAX_REQUEST_BYTES,
    MAX_RESPONSE_BYTES,
};
use crate::api_provider::ApiProvider;
use serde_json::{json, Value};

pub(super) fn auth_headers(provider: ApiProvider, key: &str) -> String {
    // The HTTP client validates the key before passing it to this function.
    match provider {
        ApiProvider::OpenAi | ApiProvider::OpenAiCompatible => {
            format!("Authorization: Bearer {key}\r\n")
        }
        ApiProvider::Anthropic => {
            format!("x-api-key: {key}\r\nanthropic-version: 2026-10-21\r\n")
        }
        ApiProvider::Gemini => format!("x-goog-api-key: {key}\r\n"),
    }
}

pub(super) fn request_body(
    provider: ApiProvider,
    model: &str,
    fast_mode: bool,
    request: &TranslationRequest,
) -> Result<Vec<u8>, ApiError> {
    if provider == ApiProvider::OpenAi {
        return super::request_body(model, fast_mode, request);
    }
    super::validate_model(model)?;
    let (instructions, input) = super::translation_prompt(request)?;
    let body = match provider {
        ApiProvider::OpenAi => unreachable!(),
        ApiProvider::OpenAiCompatible => json!({
            "model": model,
            "messages": [
                {"role": "system", "content": instructions},
                {"role": "user", "content": input}
            ],
            "stream": false,
            "max_tokens": MAX_OUTPUT_TOKENS,
            "response_format": {"type": "json_object"}
        }),
        ApiProvider::Anthropic => json!({
            "model": model,
            "system": instructions,
            "messages": [{"role": "user", "content": input}],
            "stream": false,
            "max_tokens": MAX_OUTPUT_TOKENS,
            "output_config": {
                "format": {"type": "json_schema", "schema": super::translation_schema()}
            }
        }),
        ApiProvider::Gemini => json!({
            "systemInstruction": {"parts": [{"text": instructions}]},
            "contents": [{"role": "user", "parts": [{"text": input}]}],
            "generationConfig": gemini_generation_config(model)
        }),
    };
    let bytes = serde_json::to_vec(&body).map_err(|_| invalid_response())?;
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(ApiError::new(
            &crate::i18n::text(crate::i18n::Key::M6B53566A9564),
            false,
        ));
    }
    Ok(bytes)
}

fn gemini_generation_config(model: &str) -> Value {
    // Gemini counts thinking inside maxOutputTokens. Leave room for a short
    // translation and use only thinking settings documented for exact model
    // IDs. Unknown/custom models keep their provider default; prefix matching
    // could accidentally apply text-model settings to image or audio variants.
    // https://ai.google.dev/gemini-api/docs/generate-content/thinking
    // Reviewed 2026-11-18 (Asia/Seoul).
    let mut config = json!({
        "maxOutputTokens": 4_096,
        "responseMimeType": "application/json",
        "responseJsonSchema": super::translation_schema()
    });
    let thinking = match model.strip_prefix("models/").unwrap_or(model) {
        "gemini-4-carbon"
        | "gemini-4-argon"
        | "gemini-4-hydrogen"
        | "gemini-3.8-flash"
        | "gemini-3.1-pro-preview"
        | "gemini-3-pro-preview" => Some(json!({"thinkingLevel": "low"})),
        "gemini-3.6-flash"
        | "gemini-3.5-flash-lite"
        | "gemini-3.1-flash-lite"
        | "gemini-3-flash-preview" => Some(json!({"thinkingLevel": "minimal"})),
        "gemini-2.5-pro" => Some(json!({"thinkingBudget": 128})),
        "gemini-2.5-flash" | "gemini-2.5-flash-lite" => Some(json!({"thinkingBudget": 0})),
        _ => None,
    };
    if let Some(thinking) = thinking {
        config["thinkingConfig"] = thinking;
    }
    config
}

fn invalid_response() -> ApiError {
    ApiError::new(&crate::i18n::text(crate::i18n::Key::M36B996640E45), false)
}

fn quota_error() -> ApiError {
    ApiError::new(&crate::i18n::text(crate::i18n::Key::M87B5A7D24EBB), true)
}

pub(super) fn check_status(
    provider: ApiProvider,
    status: u32,
    body: &[u8],
    retry_header: Option<&str>,
) -> Result<(), ApiError> {
    if provider == ApiProvider::OpenAi {
        return super::check_status(status, body, retry_header);
    }
    let details = (body.len() <= MAX_RESPONSE_BYTES)
        .then(|| serde_json::from_slice::<Value>(body).ok())
        .flatten();
    if status == 402
        || (provider == ApiProvider::Anthropic
            && details.as_ref().and_then(|value| {
                value
                    .pointer("/error/details/error_code")
                    .and_then(Value::as_str)
            }) == Some("enforced_spend_limit_reached"))
    {
        return Err(quota_error());
    }
    let mut result = super::check_status(status, body, retry_header);
    if let Err(error) = &mut result {
        if !error.fatal && matches!(status, 429 | 503 | 529) {
            let mut delay = super::retry_after(retry_header);
            if provider == ApiProvider::Gemini {
                if let Some(details) = details
                    .as_ref()
                    .and_then(|value| value.pointer("/error/details"))
                    .and_then(Value::as_array)
                {
                    for detail in details {
                        if detail.get("@type").and_then(Value::as_str)
                            == Some("type.googleapis.com/google.rpc.RetryInfo")
                        {
                            let seconds = detail
                                .get("retryDelay")
                                .and_then(Value::as_str)
                                .and_then(|duration| duration.strip_suffix('s'));
                            if let Some(seconds) = super::retry_after(seconds) {
                                delay = Some(delay.map_or(seconds, |current| current.max(seconds)));
                            }
                        }
                    }
                }
            }
            error.retry_after_secs = delay;
        }
    }
    result
}

fn optional_payload(value: Option<&Value>) -> bool {
    value.is_some_and(|value| !value.is_null() && !value.as_array().is_some_and(Vec::is_empty))
}

pub(super) fn parse_response(
    provider: ApiProvider,
    body: &[u8],
    request: &TranslationRequest,
) -> Result<TranslationResponse, ApiError> {
    if provider == ApiProvider::OpenAi {
        return super::parse_response(body, request);
    }
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(invalid_response());
    }
    let value: Value = serde_json::from_slice(body).map_err(|_| invalid_response())?;
    if !value.is_object() || optional_payload(value.get("error")) {
        return Err(invalid_response());
    }
    let text = match provider {
        ApiProvider::OpenAi => unreachable!(),
        ApiProvider::OpenAiCompatible => {
            let choices = value
                .get("choices")
                .and_then(Value::as_array)
                .filter(|choices| choices.len() == 1)
                .ok_or_else(invalid_response)?;
            let choice = &choices[0];
            let message = choice.get("message").ok_or_else(invalid_response)?;
            if choice.get("finish_reason").and_then(Value::as_str) != Some("stop")
                || message.get("role").and_then(Value::as_str) != Some("assistant")
                || optional_payload(message.get("refusal"))
                || optional_payload(message.get("tool_calls"))
                || optional_payload(message.get("function_call"))
            {
                return Err(invalid_response());
            }
            message
                .get("content")
                .and_then(Value::as_str)
                .ok_or_else(invalid_response)?
                .to_owned()
        }
        ApiProvider::Anthropic => {
            if value.get("type").and_then(Value::as_str) != Some("message")
                || value.get("role").and_then(Value::as_str) != Some("assistant")
                || value.get("stop_reason").and_then(Value::as_str) != Some("end_turn")
            {
                return Err(invalid_response());
            }
            let parts = value
                .get("content")
                .and_then(Value::as_array)
                .ok_or_else(invalid_response)?;
            let mut text = String::new();
            for part in parts {
                match part.get("type").and_then(Value::as_str) {
                    Some("thinking" | "redacted_thinking") => continue,
                    Some("text") => {
                        text.push_str(
                            part.get("text")
                                .and_then(Value::as_str)
                                .ok_or_else(invalid_response)?,
                        );
                    }
                    _ => return Err(invalid_response()),
                }
            }
            text
        }
        ApiProvider::Gemini => {
            if value
                .pointer("/promptFeedback/blockReason")
                .is_some_and(|reason| {
                    !reason.is_null() && reason.as_str() != Some("BLOCK_REASON_UNSPECIFIED")
                })
            {
                return Err(invalid_response());
            }
            let candidates = value
                .get("candidates")
                .and_then(Value::as_array)
                .filter(|candidates| candidates.len() == 1)
                .ok_or_else(invalid_response)?;
            let candidate = &candidates[0];
            if candidate.get("finishReason").and_then(Value::as_str) != Some("STOP") {
                return Err(invalid_response());
            }
            let content = candidate.get("content").ok_or_else(invalid_response)?;
            if content
                .get("role")
                .is_some_and(|role| role.as_str() != Some("model"))
            {
                return Err(invalid_response());
            }
            let parts = content
                .get("parts")
                .and_then(Value::as_array)
                .ok_or_else(invalid_response)?;
            let mut text = String::new();
            for part in parts {
                if [
                    "functionCall",
                    "functionResponse",
                    "executableCode",
                    "codeExecutionResult",
                    "inlineData",
                    "fileData",
                ]
                .iter()
                .any(|key| optional_payload(part.get(*key)))
                {
                    return Err(invalid_response());
                }
                if part.get("thought").and_then(Value::as_bool) == Some(true) {
                    continue;
                }
                text.push_str(
                    part.get("text")
                        .and_then(Value::as_str)
                        .ok_or_else(invalid_response)?,
                );
            }
            text
        }
    };
    let mut response = super::parse_translation_json(&text, request)?;
    let usage = |path: &str| value.pointer(path).and_then(Value::as_u64);
    match provider {
        ApiProvider::OpenAi => unreachable!(),
        ApiProvider::OpenAiCompatible => {
            response.input_tokens = usage("/usage/prompt_tokens").unwrap_or(0);
            response.output_tokens = usage("/usage/completion_tokens").unwrap_or(0);
            response.reasoning_tokens = usage("/usage/completion_tokens_details/reasoning_tokens");
        }
        ApiProvider::Anthropic => {
            response.input_tokens = usage("/usage/input_tokens")
                .unwrap_or(0)
                .saturating_add(usage("/usage/cache_creation_input_tokens").unwrap_or(0))
                .saturating_add(usage("/usage/cache_read_input_tokens").unwrap_or(0));
            response.output_tokens = usage("/usage/output_tokens").unwrap_or(0);
            response.reasoning_tokens = usage("/usage/output_tokens_details/thinking_tokens");
        }
        ApiProvider::Gemini => {
            response.input_tokens = usage("/usageMetadata/promptTokenCount").unwrap_or(0);
            response.reasoning_tokens = usage("/usageMetadata/thoughtsTokenCount");
            // Match the existing total-generated-output convention: include thoughts.
            response.output_tokens = usage("/usageMetadata/candidatesTokenCount")
                .unwrap_or(0)
                .saturating_add(response.reasoning_tokens.unwrap_or(0));
        }
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> TranslationRequest {
        TranslationRequest {
            text: "arigatou ☆".into(),
            language_a: "ko".into(),
            language_b: "ja".into(),
            glossary: "gate = 문".into(),
            romaji_japanese_input: false,
        }
    }

    fn fixture(provider: ApiProvider, translated: &str) -> Value {
        match provider {
            ApiProvider::OpenAi => json!({
                "status":"completed", "error":null, "incomplete_details":null,
                "output":[{"type":"message","role":"assistant","status":"completed",
                    "content":[{"type":"output_text","text":translated}]}],
                "usage":{"input_tokens":10,"output_tokens":3}
            }),
            ApiProvider::OpenAiCompatible => json!({
                "choices":[{"finish_reason":"stop","message":{"role":"assistant",
                    "content":translated,"refusal":null}}],
                "usage":{"prompt_tokens":10,"completion_tokens":3}
            }),
            ApiProvider::Anthropic => json!({
                "type":"message","role":"assistant","stop_reason":"end_turn",
                "content":[{"type":"text","text":translated}],
                "usage":{"input_tokens":10,"output_tokens":3}
            }),
            ApiProvider::Gemini => json!({
                "candidates":[{"finishReason":"STOP","content":{"role":"model",
                    "parts":[{"text":translated}]}}],
                "usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":3}
            }),
        }
    }

    const PROVIDERS: [ApiProvider; 4] = [
        ApiProvider::OpenAi,
        ApiProvider::OpenAiCompatible,
        ApiProvider::Anthropic,
        ApiProvider::Gemini,
    ];

    #[test]
    fn adapters_keep_openai_unchanged_and_separate_each_auth_scheme() {
        let input = request();
        assert_eq!(
            request_body(ApiProvider::OpenAi, "gpt-6-luna", true, &input).unwrap(),
            super::super::request_body("gpt-6-luna", true, &input).unwrap()
        );
        assert_eq!(
            auth_headers(ApiProvider::OpenAi, "fake-key"),
            "Authorization: Bearer fake-key\r\n"
        );
        assert_eq!(
            auth_headers(ApiProvider::OpenAiCompatible, "fake-key"),
            "Authorization: Bearer fake-key\r\n"
        );
        assert_eq!(
            auth_headers(ApiProvider::Anthropic, "fake-key"),
            "x-api-key: fake-key\r\nanthropic-version: 2026-10-21\r\n"
        );
        assert_eq!(
            auth_headers(ApiProvider::Gemini, "fake-key"),
            "x-goog-api-key: fake-key\r\n"
        );
    }

    #[test]
    fn all_adapters_preserve_the_common_prompt_data_routing_and_sender_choice() {
        let mut input = request();
        for provider in PROVIDERS {
            for enabled in [false, true] {
                input.romaji_japanese_input = enabled;
                let body: Value = serde_json::from_slice(
                    &request_body(provider, "custom-model", true, &input).unwrap(),
                )
                .unwrap();
                let (prompt, data) = match provider {
                    ApiProvider::OpenAi => (
                        &body["instructions"],
                        &body["input"][0]["content"][0]["text"],
                    ),
                    ApiProvider::OpenAiCompatible => (
                        &body["messages"][0]["content"],
                        &body["messages"][1]["content"],
                    ),
                    ApiProvider::Anthropic => (&body["system"], &body["messages"][0]["content"]),
                    ApiProvider::Gemini => (
                        &body["systemInstruction"]["parts"][0]["text"],
                        &body["contents"][0]["parts"][0]["text"],
                    ),
                };
                assert_eq!(
                    prompt
                        .as_str()
                        .unwrap()
                        .contains(super::super::ROMAJI_INSTRUCTIONS),
                    enabled
                );
                let data: Value = serde_json::from_str(data.as_str().unwrap()).unwrap();
                assert_eq!(
                    data,
                    json!({"language_a":"ko","language_b":"ja","glossary":input.glossary,"chat_text":input.text})
                );
                if provider != ApiProvider::OpenAi {
                    for field in [
                        "reasoning",
                        "thinking",
                        "temperature",
                        "top_p",
                        "top_k",
                        "service_tier",
                        "speed",
                    ] {
                        assert!(body.get(field).is_none(), "{field}");
                    }
                    assert_eq!(
                        request_body(provider, "custom-model", false, &input).unwrap(),
                        request_body(provider, "custom-model", true, &input).unwrap()
                    );
                }
                for (tag, source, target) in [
                    ("a", "ko", "ja"),
                    ("b", "ja", "ko"),
                    ("other", "other", "ko"),
                ] {
                    let json = json!({"s":tag,"t":"완성된 번역 ☆"}).to_string();
                    let response = parse_response(
                        provider,
                        &serde_json::to_vec(&fixture(provider, &json)).unwrap(),
                        &input,
                    )
                    .unwrap();
                    assert_eq!(
                        (
                            response.source_language.as_str(),
                            response.target_language.as_str()
                        ),
                        (source, target)
                    );
                    assert_eq!(response.text, "완성된 번역 ☆");
                    assert_eq!((response.input_tokens, response.output_tokens), (10, 3));
                }
            }
        }
    }

    #[test]
    fn native_structured_output_and_compatible_json_mode_have_the_expected_shapes() {
        let input = request();
        let body = |provider| -> Value {
            serde_json::from_slice(&request_body(provider, "custom/model", false, &input).unwrap())
                .unwrap()
        };
        let compatible = body(ApiProvider::OpenAiCompatible);
        assert_eq!(compatible["model"], "custom/model");
        assert_eq!(compatible["response_format"], json!({"type":"json_object"}));
        let claude = body(ApiProvider::Anthropic);
        assert_eq!(
            claude["output_config"]["format"]["schema"],
            super::super::translation_schema()
        );
        let gemini = body(ApiProvider::Gemini);
        assert_eq!(
            gemini["generationConfig"]["responseMimeType"],
            "application/json"
        );
        assert_eq!(
            gemini["generationConfig"]["responseJsonSchema"],
            super::super::translation_schema()
        );
    }

    #[test]
    fn gemini_thinking_controls_follow_exact_supported_models_and_do_not_leak() {
        let input = request();
        for (model, expected) in [
            ("gemini-4-hydrogen", json!({"thinkingLevel":"low"})),
            ("gemini-3.8-flash", json!({"thinkingLevel":"low"})),
            ("gemini-3.6-flash", json!({"thinkingLevel":"minimal"})),
            (
                "models/gemini-3.1-flash-lite",
                json!({"thinkingLevel":"minimal"}),
            ),
            ("gemini-2.5-pro", json!({"thinkingBudget":128})),
            ("models/gemini-2.5-flash", json!({"thinkingBudget":0})),
        ] {
            let body: Value = serde_json::from_slice(
                &request_body(ApiProvider::Gemini, model, false, &input).unwrap(),
            )
            .unwrap();
            assert_eq!(body["generationConfig"]["thinkingConfig"], expected);
            assert_eq!(body["generationConfig"]["maxOutputTokens"], 4_096);
            assert_eq!(
                body["generationConfig"]["responseJsonSchema"],
                super::super::translation_schema()
            );
        }
        for model in [
            "custom-model",
            "gemini-3.1-flash-lite-image",
            "gemini-4-hydrogen-unknown",
        ] {
            let body: Value = serde_json::from_slice(
                &request_body(ApiProvider::Gemini, model, false, &input).unwrap(),
            )
            .unwrap();
            assert!(body["generationConfig"].get("thinkingConfig").is_none());
        }
        for provider in [ApiProvider::OpenAiCompatible, ApiProvider::Anthropic] {
            let body: Value = serde_json::from_slice(
                &request_body(provider, "gemini-4-hydrogen", false, &input).unwrap(),
            )
            .unwrap();
            assert!(body.get("generationConfig").is_none());
            assert_eq!(body["max_tokens"], MAX_OUTPUT_TOKENS);
        }
    }

    #[test]
    fn adapters_reject_bad_schemas_and_oversized_data_without_echoing_response_text() {
        let input = request();
        for provider in PROVIDERS {
            for text in [
                "not-json secret-value",
                r#"{"s":"x","t":"secret-value"}"#,
                r#"{"s":"a","t":""}"#,
                r#"{"s":"a","t":"secret-value","extra":true}"#,
            ] {
                let error = parse_response(
                    provider,
                    &serde_json::to_vec(&fixture(provider, text)).unwrap(),
                    &input,
                )
                .unwrap_err();
                assert!(!error.message.contains("secret-value"));
            }
            assert!(parse_response(provider, &vec![b' '; MAX_RESPONSE_BYTES + 1], &input).is_err());
        }
    }

    #[test]
    fn third_party_adapters_do_not_publish_refusals_partial_outputs_or_tool_calls() {
        let input = request();
        let translated = r#"{"s":"b","t":"고마워"}"#;
        for reason in ["length", "content_filter", "tool_calls"] {
            let mut value = fixture(ApiProvider::OpenAiCompatible, translated);
            value["choices"][0]["finish_reason"] = json!(reason);
            assert!(parse_response(
                ApiProvider::OpenAiCompatible,
                &serde_json::to_vec(&value).unwrap(),
                &input
            )
            .is_err());
        }
        for field in ["refusal", "tool_calls", "function_call"] {
            let mut value = fixture(ApiProvider::OpenAiCompatible, translated);
            value["choices"][0]["message"][field] = json!({"unexpected":"data"});
            assert!(parse_response(
                ApiProvider::OpenAiCompatible,
                &serde_json::to_vec(&value).unwrap(),
                &input
            )
            .is_err());
        }
        for reason in ["refusal", "max_tokens", "tool_use", "pause_turn"] {
            let mut value = fixture(ApiProvider::Anthropic, translated);
            value["stop_reason"] = json!(reason);
            assert!(parse_response(
                ApiProvider::Anthropic,
                &serde_json::to_vec(&value).unwrap(),
                &input
            )
            .is_err());
        }
        let mut tool = fixture(ApiProvider::Anthropic, translated);
        tool["content"]
            .as_array_mut()
            .unwrap()
            .push(json!({"type":"tool_use","name":"unexpected"}));
        assert!(parse_response(
            ApiProvider::Anthropic,
            &serde_json::to_vec(&tool).unwrap(),
            &input
        )
        .is_err());
        for reason in ["MAX_TOKENS", "SAFETY", "RECITATION", "OTHER"] {
            let mut value = fixture(ApiProvider::Gemini, translated);
            value["candidates"][0]["finishReason"] = json!(reason);
            assert!(parse_response(
                ApiProvider::Gemini,
                &serde_json::to_vec(&value).unwrap(),
                &input
            )
            .is_err());
        }
        let mut blocked = fixture(ApiProvider::Gemini, translated);
        blocked["promptFeedback"] = json!({"blockReason":"SAFETY"});
        assert!(parse_response(
            ApiProvider::Gemini,
            &serde_json::to_vec(&blocked).unwrap(),
            &input
        )
        .is_err());
        let mut tool = fixture(ApiProvider::Gemini, translated);
        tool["candidates"][0]["content"]["parts"]
            .as_array_mut()
            .unwrap()
            .push(json!({"functionCall":{"name":"unexpected"}}));
        assert!(parse_response(
            ApiProvider::Gemini,
            &serde_json::to_vec(&tool).unwrap(),
            &input
        )
        .is_err());
        for provider in [ApiProvider::OpenAiCompatible, ApiProvider::Gemini] {
            let mut value = fixture(provider, translated);
            let key = if provider == ApiProvider::Gemini {
                "candidates"
            } else {
                "choices"
            };
            let first = value[key][0].clone();
            value[key].as_array_mut().unwrap().push(first);
            assert!(
                parse_response(provider, &serde_json::to_vec(&value).unwrap(), &input).is_err()
            );
            value[key] = json!([]);
            assert!(
                parse_response(provider, &serde_json::to_vec(&value).unwrap(), &input).is_err()
            );
        }
    }

    #[test]
    fn thought_parts_are_not_posted_and_provider_usage_is_normalized() {
        let input = request();
        let mut claude = fixture(ApiProvider::Anthropic, r#"{"s":"b","t":"고마워"}"#);
        claude["content"]
            .as_array_mut()
            .unwrap()
            .insert(0, json!({"type":"thinking","thinking":"private reasoning"}));
        claude["usage"]["cache_read_input_tokens"] = json!(4);
        let parsed = parse_response(
            ApiProvider::Anthropic,
            &serde_json::to_vec(&claude).unwrap(),
            &input,
        )
        .unwrap();
        assert_eq!(parsed.text, "고마워");
        assert_eq!(parsed.input_tokens, 14);
        let mut gemini = fixture(ApiProvider::Gemini, r#"{"s":"b","t":"고마워"}"#);
        gemini["candidates"][0]["content"]["parts"]
            .as_array_mut()
            .unwrap()
            .insert(0, json!({"thought":true,"text":"private reasoning"}));
        gemini["usageMetadata"]["thoughtsTokenCount"] = json!(2);
        let parsed = parse_response(
            ApiProvider::Gemini,
            &serde_json::to_vec(&gemini).unwrap(),
            &input,
        )
        .unwrap();
        assert_eq!(parsed.text, "고마워");
        assert_eq!(parsed.reasoning_tokens, Some(2));
        assert_eq!(parsed.output_tokens, 5);
    }

    #[test]
    fn third_party_statuses_distinguish_quota_and_wait_hints_without_retries() {
        for provider in PROVIDERS {
            let error = check_status(provider, 429, b"{}", Some("12.1")).unwrap_err();
            assert!(!error.fatal);
            assert_eq!(error.retry_after_secs, Some(13));
            assert!(
                check_status(provider, 401, b"secret-key", None)
                    .unwrap_err()
                    .fatal
            );
            assert!(
                check_status(provider, 302, b"secret-key", None)
                    .unwrap_err()
                    .fatal
            );
        }
        let spend = json!({"error":{"details":{"error_code":"enforced_spend_limit_reached"}}});
        assert!(
            check_status(
                ApiProvider::Anthropic,
                429,
                &serde_json::to_vec(&spend).unwrap(),
                None
            )
            .unwrap_err()
            .fatal
        );
        assert!(
            check_status(ApiProvider::Gemini, 402, b"{}", None)
                .unwrap_err()
                .fatal
        );
        let retry = json!({"error":{"details":[{"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"40.1s"}]}});
        let error = check_status(
            ApiProvider::Gemini,
            429,
            &serde_json::to_vec(&retry).unwrap(),
            Some("12"),
        )
        .unwrap_err();
        assert_eq!(error.retry_after_secs, Some(41));
        let error = check_status(ApiProvider::Anthropic, 529, b"{}", Some("20")).unwrap_err();
        assert!(!error.fatal);
        assert_eq!(error.retry_after_secs, Some(20));
    }
}
