# Sideglow — API 제공사 설정

대상 버전: **0.5.0-preview.7** · 작성·규격 확인: **2026-11-18(Asia/Seoul)**

Sideglow는 **OpenAI, OpenAI 호환 서비스, Anthropic, Gemini** 중 하나로 채널의 새 텍스트를 번역할 수 있습니다. 번역을 맡은 한 사람만 키와 모델을 준비하고, 친구들은 키 없이 같은 채널에 게시된 결과를 받습니다. Discord 연결과 일반 번역 사용법은 [SETUP.md](SETUP.md), [AI-TRANSLATION.md](AI-TRANSLATION.md)를 참고하세요.

**한 PC에는 활성 번역 설정 한 개만 저장합니다.** 제공사·API 기본 주소·키·모델·언어·게임 용어를 모든 채널 탭이 함께 사용하며 현재 선택한 탭만 번역합니다. 제공사별 여러 프로필이나 키 목록, 자동 모델 선택, 오류 시 다른 제공사로 전환하는 기능은 없습니다.

## 설정 순서

1. 번역 담당 PC에서 **설정 → AI 번역 설정…**을 엽니다.
2. **API 제공사**에서 **OpenAI / OpenAI 호환 서비스 / Anthropic / Gemini** 중 하나를 고릅니다.
3. **API 기본 주소**를 확인합니다. 공식 OpenAI·Anthropic·Gemini는 주소가 고정되어 읽기 전용입니다. **OpenAI 호환 서비스**는 해당 서비스에서 안내한 HTTPS 기본 주소를 직접 넣습니다.
4. **번역 API 키 · 이 Windows 계정에 보호 저장**에 선택한 연결용 키를 넣습니다. 제공사·주소를 먼저 정한 다음 키를 입력하세요. 입력 도중 제공사나 실제 대상 주소가 바뀌면 입력한 키를 비웁니다.
5. **번역 모델** 아래 **모델 ID · 직접 입력 가능**에 그 제공사에서 사용할 수 있는 모델 ID를 입력합니다. OpenAI만 기존 모델 프리셋을 제공합니다. 다른 제공사를 고를 때 모델 이름을 자동 추천·변환하지 않으므로 이전 OpenAI 모델명이 남아 있다면 직접 바꿉니다.
6. **언어 A / 언어 B**, 필요한 게임 용어를 설정하고 **이 PC에서 공용 번역 실행**을 켭니다. 같은 채널의 담당자는 한 명만 켭니다.
7. AI 창의 **확인**을 누르고 기본 설정의 **저장**까지 마칩니다. 확인만 누른 뒤 기본 설정을 취소하면 적용되지 않습니다.
8. 새 메시지로 동작을 확인합니다. 창을 여는 것만으로 API를 호출하거나 키를 시험하지 않으며, 설정 저장도 계정의 실제 사용 권한까지 확인하는 절차는 아닙니다.

친구들은 **이 PC에서 공용 번역 실행**을 끄고 **받은 AI 번역 표시**를 켭니다. 번역 담당 기능은 B 직접 연결에서 사용하며 기존 중계 모드에서는 실행하지 않습니다.

## 제공사별 연결 조건

| 화면의 제공사 | API 기본 주소 | 필요한 키·API 방식 | 모델 조건 |
|---|---|---|---|
| **OpenAI** | `https://api.openai.com/v1` · 고정 | OpenAI API 키, Bearer 인증, Responses API | Responses와 구조화된 JSON 출력 지원. 기존 프리셋 또는 ID 직접 입력 |
| **OpenAI 호환 서비스** | 서비스에서 받은 HTTPS 기본 주소 · 직접 입력 | 해당 서비스 키, Bearer 인증, Chat Completions | `response_format: {"type":"json_object"}`와 `max_tokens` 등을 지원하는 모델 ID 직접 입력 |
| **Anthropic** | `https://api.anthropic.com/v1` · 고정 | 단일 workspace에 연결된 Claude API 키, Messages API | `output_config.format`의 JSON schema를 지원하는 모델 ID 직접 입력 |
| **Gemini** | `https://generativelanguage.googleapis.com/v1beta` · 고정 | Google AI Studio에서 준비한 Gemini API 키, GenerateContent API | `application/json` 응답과 `responseJsonSchema`를 지원하는 모델 ID 직접 입력 |

Sideglow는 모델 사용 권한과 요금을 조회하지 않습니다. 각 제공사에서 해당 키로 사용할 수 있는 모델 ID를 확인하세요. **공식 모델 목록 열기**는 공식 제공사의 문서를 여는 버튼이며 내 계정의 사용 가능 목록을 조회하는 버튼이 아닙니다. 호환 서비스에서는 이 버튼이 비활성화되므로 해당 서비스 문서를 확인합니다.

### OpenAI

기존 OpenAI 설정과 저장된 키는 공식 OpenAI 주소에서 계속 사용할 수 있습니다. 새 설치의 기본 모델은 기존과 같은 `gpt-6-luna`이며 다른 ID도 직접 입력할 수 있습니다. 모델 프리셋·추론 설정·요금 설명은 [AI 번역 안내의 OpenAI 모델 절](AI-TRANSLATION.md#openai-모델-목록과-직접-입력)에 있습니다.

요청은 고정된 `/v1/responses`로 보내며 기존 구조화된 JSON 출력, `store:false`, 표준/Fast 선택을 유지합니다. 지역별 OpenAI 주소나 별도의 OpenAI 프록시를 이 공식 모드에 입력할 수 없습니다. **OpenAI 호환 서비스**는 별도의 Chat Completions 방식이므로 공식 Responses의 주소만 바꾸는 모드로 사용하지 마세요.

### OpenAI 호환 서비스

예를 들어 서비스가 최종 요청 주소를 다음과 같이 안내했다면:

```text
https://api.example.com/v1/chat/completions
```

**API 기본 주소**에는 아래까지만 넣습니다. 예시 주소는 실제 서비스를 지정한 것이 아니므로 자신이 사용하는 주소로 바꾸세요.

```text
https://api.example.com/v1
```

Sideglow가 뒤에 `/chat/completions`를 붙입니다. 서비스가 별도의 기본 경로를 안내하면 `/v1`을 임의로 추가하지 말고 그 경로를 사용합니다. `http://`, 로그인 정보가 포함된 URL, 쿼리·fragment·API 키와 최종 요청 경로가 포함된 주소는 받지 않습니다. HTTPS 인증서 검증을 유지하고 리다이렉트는 따라가지 않습니다.

이 모드가 요구하는 계약은 다음과 같습니다.

- `Authorization: Bearer <API 키>` 인증
- `POST <API 기본 주소>/chat/completions`
- `model`, system/user `messages`, `stream:false`, `max_tokens:1600`, `response_format:{"type":"json_object"}` 요청
- 응답의 한 개 `choices` 항목, `finish_reason:"stop"`, assistant 메시지의 문자열 `content`
- `content` 안에 언어 태그와 번역문을 담은 `{"s":"b","t":"번역문"}` 형태의 정확한 JSON 객체. `s`의 값은 `a`, `b`, `other` 중 하나입니다.

“OpenAI 호환”이라는 설명만으로 모든 서비스·모델이 위 항목을 지원하는 것은 아닙니다. `max_completion_tokens`만 받는 모델, JSON mode를 지원하지 않는 모델, 별도 인증 헤더가 필요한 서비스에는 이 어댑터가 맞지 않습니다. 실패했을 때 JSON mode나 제한을 자동으로 빼서 재요청하지 않습니다. 모델 ID의 `조직/모델` 형태는 지원하지만 실제 제공 여부는 서비스에서 확인해야 합니다.

### Anthropic / Claude

**단일 workspace에 연결된 API 키**를 준비합니다. 이 방식은 키에 연결된 workspace를 사용하므로 별도 workspace 선택 헤더가 필요하지 않습니다. Sideglow는 `x-api-key`와 `anthropic-version: 2026-10-21`을 보내고 `/v1/messages`의 `output_config.format`으로 JSON schema를 지정합니다.

**조직 전체 등 여러 workspace를 대상으로 하는 키에 필요한 `anthropic-workspace-id` 입력은 지원하지 않습니다.** 그러한 키를 그대로 넣고 workspace를 선택하는 기능은 없으므로 사용할 workspace에 연결된 키를 발급해 입력하세요. AWS Bedrock·Vertex AI 같은 별도 클라우드 경유 인증도 이 Anthropic 모드의 대상이 아닙니다. 키 범위와 헤더 조건은 [Claude 공식 API 개요](https://platform.claude.com/docs/en/api/overview)를 참고하세요.

### Google Gemini

[Google AI Studio의 API 키 화면](https://aistudio.google.com/apikey)에서 사용할 프로젝트의 Gemini API 키를 준비합니다. 키는 `x-goog-api-key` 헤더로 보내며 URL에 넣지 않습니다. 요청 경로는 `/v1beta/models/{모델 ID}:generateContent`입니다.

모델 ID는 해당 API에서 안내하는 값을 직접 입력합니다. `models/` 접두부를 포함한 ID도 한 번 제거해 처리합니다. 모델·프로젝트·지역이 들어간 전체 URL이나 Vertex 리소스 경로는 모델 입력란에 넣지 않습니다. **Vertex AI IAM·OAuth 토큰·서비스 계정 JSON 인증은 지원하지 않습니다.** Gemini API 키와 사용할 프로젝트의 권한·제한 조건은 [공식 키 안내](https://aistudio.google.com/docs/api-key)를 확인하세요.

Gemini 요청의 출력 상한은 **4,096토큰**입니다. 생각 토큰 때문에 번역 JSON이 잘릴 가능성을 줄이도록 공식 문서에서 지원을 확인한 정확한 모델 ID에만 번역용 낮은 thinking 설정을 적용합니다. 등록하지 않은 모델 ID에는 추측한 thinking 설정을 보내지 않고 제공사 기본값을 사용합니다. 이 조정은 모든 모델·문장의 성공을 보장하지 않으며, 응답이 상한에 걸려 불완전하면 게시하지 않습니다. 다른 세 방식의 출력 상한은 기존 **1,600토큰**을 유지하고 Discord에 게시할 수 있는 번역 길이 제한도 유지합니다. 실제 Gemini 호출로 성공률·품질·지연을 측정한 것은 아닙니다.

현재 자동으로 적용하는 값은 아래와 같습니다. [공식 thinking 지원 표](https://aistudio.google.com/docs/thinking?generate-content=true)와 대조했으며 구현은 [providers.rs](../src/openai/providers.rs)의 `gemini_generation_config`에 있습니다.

| 정확한 모델 ID | `thinkingConfig`에 넣는 값 |
|---|---|
| `gemini-4-carbon`, `gemini-4-argon`, `gemini-4-hydrogen` | `thinkingLevel: "low"` |
| `gemini-3.8-flash`, `gemini-3.1-pro-preview`, `gemini-3-pro-preview` | `thinkingLevel: "low"` |
| `gemini-3.6-flash`, `gemini-3.5-flash-lite`, `gemini-3.1-flash-lite`, `gemini-3-flash-preview` | `thinkingLevel: "minimal"` |
| `gemini-2.5-pro` | `thinkingBudget: 128` |
| `gemini-2.5-flash`, `gemini-2.5-flash-lite` | `thinkingBudget: 0` |

`models/` 접두부를 제거한 뒤 ID 전체가 일치할 때만 적용합니다. 이미지·오디오 모델, 다른 snapshot ID나 앞으로 추가되는 모델에 이름의 일부만 보고 같은 값을 넣지 않습니다. 사용할 모델 ID는 여전히 직접 입력하며 실제 접근 가능 여부는 해당 계정에서 확인합니다.

## 키 유지·교체와 저장

키 입력란은 저장된 키를 다시 표시하지 않습니다. **제공사와 정규화된 API 기본 주소가 같으면 키 입력란을 비워 두어 기존 키를 유지**할 수 있습니다. 같은 제공사·주소에서 모델, 언어 A/B, 게임 용어를 바꾸는 경우도 여기에 해당합니다.

**제공사나 실제 API 기본 주소가 달라지면 새 연결에 사용할 키를 입력**해야 합니다. 주소의 호스트 대소문자, 기본 443 포트 표기, 끝의 `/`처럼 정규화 후 같은 주소인 차이는 같은 대상으로 처리합니다. 호스트·포트·기본 경로가 달라지거나 제공사 선택이 바뀌면 다른 대상입니다. 같은 URL을 입력했더라도 **OpenAI**와 **OpenAI 호환 서비스**는 서로 다른 설정입니다.

키는 대상 제공사·주소와 함께 Windows DPAPI로 보호하고 실행 시 그 대상과 일치하는지 다시 확인합니다. 다른 대상의 저장 키를 자동 전송하지 않습니다. 새 연결과 키를 저장하면 이전 키를 제공사별로 따로 보관하지 않으므로, 이전 제공사로 돌아갈 때도 그 연결용 키를 다시 입력해야 합니다. 기존 버전의 OpenAI 키는 공식 OpenAI 기본 주소에만 연결합니다.

**저장된 키 삭제 → 확인 → 기본 설정에서 저장**하면 이 PC의 키를 지우고 공용 번역 실행도 끕니다. 설정을 읽거나 AI 창을 여는 것만으로 저장 키를 복호화하지 않습니다. 키는 친구용 연결 코드·파일에 포함되지 않습니다.

## Fast·개인별 로마자 옵션·캐시

모델 프리셋과 **Fast 처리 사용 · 목록 모델은 표준 요금의 2배**는 **OpenAI 전용**입니다. 다른 제공사를 고르면 Fast는 해제·비활성화되고 **Fast 처리 · OpenAI 전용**으로 표시합니다. 타사에 OpenAI용 reasoning·service tier나 별도의 temperature 설정을 보내지 않습니다. 최근 번역 시간의 타사 처리 등급은 **처리 방식: 제공사 기본값**으로 표시하며 OpenAI의 표준/Fast 등급으로 추측하지 않습니다.

**내 메시지의 로마자 일본어 판별**은 네 제공사에 공통 적용합니다. 기본값은 해제이며 각 발신자가 자신의 기본 설정에서 선택합니다. 체크한 사람의 Sideglow 메시지이고 언어 쌍에 일본어가 있을 때만 기존 한 번의 번역 요청에 추가 지침을 넣습니다. 일반 한국어·일본어 번역, 원문·닉네임·현지 시각 표시와 과거 메시지를 자동 재번역하지 않는 정책은 유지됩니다.

완료된 결과 캐시는 같은 제공사·주소·키·모델·언어 등 실행 설정에만 속합니다. 원문이 같아도 발신자의 로마자 옵션이 다르면 구분합니다. 제공사·주소·모델·키를 바꾸면 이전 결과를 새 모델 결과로 재사용하지 않습니다. 공급자를 늘려도 한 메시지에 여러 API를 동시에 호출하거나 오류 후 다른 유료 서비스로 넘기지 않습니다.

## 오류와 확인 범위

| 상황 | 확인할 내용 |
|---|---|
| 키가 현재 제공사·주소와 다름 | 제공사·주소를 먼저 선택하고 그 연결용 키를 다시 입력합니다. 모델만 바꿀 때는 주소와 제공사가 유지됐는지 확인합니다. |
| 인증·권한·모델·잔액 오류 | 해당 제공사에서 키 범위, 모델 ID·권한, 프로젝트·잔액을 확인합니다. 오류를 고친 뒤 설정을 저장하거나 번역을 껐다 켭니다. |
| 호환 API가 400·404 등을 반환 | 기본 주소와 `/chat/completions` 중복 여부, JSON mode·`max_tokens` 지원을 확인합니다. 임의 추가 헤더나 다른 API 형식은 지원하지 않습니다. |
| 요청 제한·일시 장애 | 제한된 서버 대기 힌트를 반영합니다. 이미 실패하거나 대기 중 건너뛴 원문을 자동으로 재요청하지 않습니다. |
| 응답이 거절·차단·잘림·도구 호출·잘못된 JSON임 | 해당 결과를 채널에 게시하지 않습니다. 원시 오류 본문이나 원문을 오류 안내에 복사하지 않습니다. |

공식 문서와 로컬 요청·응답 fixture로 어댑터를 검증했습니다. **이번 작업에서는 실제 타사 API 키로 서비스를 호출하지 않았습니다.** 정상 응답, 언어 방향·로마자 옵션 보존, 잘못된 JSON, 거절·잘림·도구 호출, 응답 크기 제한, 인증 분리와 오류 처리를 모의 테스트로 확인했습니다. 실제 계정의 모델 권한·인증 성공·번역 품질·속도는 별도 확인이 필요합니다. 통합 검사와 빌드 결과는 [VALIDATION.md](VALIDATION.md)에 기록합니다.

## 공식 규격 참고

아래는 2026-11-18(Asia/Seoul)에 확인한 공식 문서입니다. 모델의 가용성과 이용 조건은 각 제공사에서 확인하세요.

- OpenAI: [Responses 생성](https://developers.openai.com/api/reference/resources/responses/methods/create), [Chat Completions 생성](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create)
- Anthropic: [API 개요·인증과 workspace](https://platform.claude.com/docs/en/api/overview), [Messages 생성](https://platform.claude.com/docs/en/api/messages/create), [구조화된 출력](https://platform.claude.com/docs/en/build-with-claude/structured-outputs)
- Gemini: [API 키](https://aistudio.google.com/docs/api-key), [GenerateContent 요청·응답 규격](https://aistudio.google.com/api-reference/generate-content/generate-content), [thinking 설정](https://aistudio.google.com/docs/thinking?generate-content=true)
