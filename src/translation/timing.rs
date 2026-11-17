// SPDX-License-Identifier: MPL-2.0
//! Local translation latency measurements. Never contains source chat, channel
//! identifiers, URLs or credentials.
use crate::{
    api_provider::ApiProvider,
    openai::{ApiTiming, ServiceTier},
};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct TranslationTiming {
    pub model: String,
    pub provider: ApiProvider,
    pub queue_ms: u64,
    pub api_ms: u64,
    pub verify_ms: u64,
    pub post_ms: u64,
    pub worker_ms: u64,
    pub ui_queue_ms: u64,
    pub cache_hit: bool,
    pub fast_requested: bool,
    pub service_tier: Option<ServiceTier>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub api_phases: Option<ApiTiming>,
    pub succeeded: bool,
    pub(super) finished_at: Instant,
}

pub(super) fn milliseconds(value: Duration) -> u64 {
    value.as_millis().min(u64::MAX as u128) as u64
}

impl TranslationTiming {
    pub(super) fn new(model: &str, queued: Instant, started: Instant) -> Self {
        Self {
            model: model.into(),
            provider: ApiProvider::OpenAi,
            queue_ms: milliseconds(started.saturating_duration_since(queued)),
            api_ms: 0,
            verify_ms: 0,
            post_ms: 0,
            worker_ms: 0,
            ui_queue_ms: 0,
            cache_hit: false,
            fast_requested: false,
            service_tier: None,
            input_tokens: None,
            output_tokens: None,
            reasoning_tokens: None,
            api_phases: None,
            succeeded: false,
            finished_at: started,
        }
    }

    pub fn total_ms(&self) -> u64 {
        self.queue_ms
            .saturating_add(self.worker_ms)
            .saturating_add(self.ui_queue_ms)
    }

    pub fn summary(&self) -> String {
        {
            let __ui_arg0 = &(if self.succeeded {
                crate::i18n::text(crate::i18n::Key::M35481991FBE6)
            } else {
                crate::i18n::text(crate::i18n::Key::M1A9EFAF5BE03)
            });
            let __ui_arg1 = &(self.total_ms() as f64 / 1000.0);
            let __ui_arg2 = &(self.queue_ms as f64 / 1000.0);
            let __ui_arg3 = &(if self.cache_hit {
                crate::i18n::text(crate::i18n::Key::M1910F074F162)
            } else {
                "API".to_string()
            });
            let __ui_arg4 = &(self.api_ms as f64 / 1000.0);
            let __ui_arg5 = &(self.verify_ms.saturating_add(self.post_ms) as f64 / 1000.0);
            crate::i18n::format(
                crate::i18n::Key::M01292A1CDBED,
                &[
                    ("arg0", __ui_arg0.to_string()),
                    ("arg1", format!("{:.2}", __ui_arg1)),
                    ("arg2", format!("{:.2}", __ui_arg2)),
                    ("arg3", __ui_arg3.to_string()),
                    ("arg4", format!("{:.2}", __ui_arg4)),
                    ("arg5", format!("{:.2}", __ui_arg5)),
                ],
            )
        }
    }

    pub fn details(&self) -> String {
        let tier_detail = if self.cache_hit {
            crate::i18n::text(crate::i18n::Key::MCF440BC5574A)
        } else if self.provider != ApiProvider::OpenAi {
            crate::i18n::text(crate::i18n::Key::TranslationProviderDefaultTier)
        } else {
            {
                let __ui_arg0 = &(if self.fast_requested {
                    "Fast".to_string()
                } else {
                    crate::i18n::text(crate::i18n::Key::M6F8E18710668)
                });
                let __ui_arg1 = &(self.service_tier.map_or(
                    crate::i18n::text(crate::i18n::Key::M937EC6F24479),
                    ServiceTier::label,
                ));
                crate::i18n::format(
                    crate::i18n::Key::M13FFAF0393E7,
                    &[
                        ("arg0", __ui_arg0.to_string()),
                        ("arg1", __ui_arg1.to_string()),
                    ],
                )
            }
        };
        let api_detail = if self.cache_hit {
            crate::i18n::text(crate::i18n::Key::MF84A5062F8E7)
        } else if let Some(phases) = self.api_phases {
            let server = phases.server_processing_ms.map_or_else(
                || crate::i18n::text(crate::i18n::Key::M539412B3E17B),
                |value| format!("{value} ms"),
            );
            let reasoning = self.reasoning_tokens.map_or_else(
                || crate::i18n::text(crate::i18n::Key::M539412B3E17B),
                |value| value.to_string(),
            );
            {
                let __ui_arg0 = &(phases.send_ms);
                let __ui_arg1 = &(phases.wait_ms);
                let __ui_arg2 = &(phases.read_ms);
                let __ui_arg3 = &(server);
                let __ui_arg4 = &(self.input_tokens.unwrap_or(0));
                let __ui_arg5 = &(self.output_tokens.unwrap_or(0));
                let __ui_arg6 = &(reasoning);
                crate::i18n::format(
                    crate::i18n::Key::M68397BB852B6,
                    &[
                        ("arg0", format!("{}", __ui_arg0)),
                        ("arg1", format!("{}", __ui_arg1)),
                        ("arg2", format!("{}", __ui_arg2)),
                        ("arg3", __ui_arg3.to_string()),
                        ("arg4", format!("{}", __ui_arg4)),
                        ("arg5", format!("{}", __ui_arg5)),
                        ("arg6", __ui_arg6.to_string()),
                    ],
                )
            }
        } else {
            crate::i18n::text(crate::i18n::Key::MA978A59F27CB)
        };
        {
            let __ui_arg0 = &(if self.succeeded {
                crate::i18n::text(crate::i18n::Key::M22CF45EDA132)
            } else {
                crate::i18n::text(crate::i18n::Key::M9BD3F426E027)
            });
            let __ui_arg1 = &format!("{} · {}", self.provider.label(), self.model);
            let __ui_arg2 = &(tier_detail);
            let __ui_arg3 = &(self.total_ms());
            let __ui_arg4 = &(self.queue_ms);
            let __ui_arg5 = &(self.api_ms);
            let __ui_arg6 = &(self.verify_ms);
            let __ui_arg7 = &(self.post_ms);
            let __ui_arg8 = &(self.ui_queue_ms);
            let __ui_arg9 = &(api_detail);
            crate::i18n::format(
                crate::i18n::Key::M26EE12D2761D,
                &[
                    ("arg0", __ui_arg0.to_string()),
                    ("arg1", __ui_arg1.to_string()),
                    ("arg2", __ui_arg2.to_string()),
                    ("arg3", format!("{}", __ui_arg3)),
                    ("arg4", format!("{}", __ui_arg4)),
                    ("arg5", format!("{}", __ui_arg5)),
                    ("arg6", format!("{}", __ui_arg6)),
                    ("arg7", format!("{}", __ui_arg7)),
                    ("arg8", format!("{}", __ui_arg8)),
                    ("arg9", __ui_arg9.to_string()),
                ],
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measured_total_counts_queue_and_ui_once_not_server_subset() {
        let now = Instant::now();
        let mut timing =
            TranslationTiming::new("example-model", now - Duration::from_millis(500), now);
        timing.worker_ms = 4800;
        timing.api_ms = 4000;
        timing.verify_ms = 300;
        timing.post_ms = 500;
        timing.ui_queue_ms = 20;
        timing.api_phases = Some(ApiTiming {
            server_processing_ms: Some(3900),
            ..Default::default()
        });
        assert_eq!(timing.total_ms(), 5320);
        assert!(timing
            .details()
            .contains("서버가 보고한 처리 시간: 3900 ms"));
        assert!(timing.details().contains("추론 토큰: 미보고"));
        assert!(timing
            .details()
            .contains("처리 방식: 요청 표준 · 실제 미보고"));
        timing.fast_requested = true;
        timing.service_tier = Some(ServiceTier::Standard);
        assert!(timing
            .details()
            .contains("처리 방식: 요청 Fast · 실제 표준"));
        assert!(timing
            .details()
            .contains("네트워크·추론을 분리한 값이 아닙니다"));
        timing.cache_hit = true;
        assert!(timing
            .details()
            .contains("새 API 요청·토큰 사용은 없습니다"));
        assert!(!timing.details().contains("요청 Fast"));
        timing.cache_hit = false;
        timing.provider = ApiProvider::Gemini;
        assert!(timing.details().contains("Gemini · example-model"));
        assert!(timing.details().contains("처리 방식: 제공사 기본값"));
        assert!(!timing.details().contains("요청 Fast"));
    }
}
