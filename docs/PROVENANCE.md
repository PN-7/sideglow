# Sideglow 출처·외부 구성 요소 라이선스 점검

점검일: 2026-11-17 (Asia/Seoul)

## 결론

확인된 외부 구성 요소에서 **Sideglow 자체 코드에 MPL 2.0을 적용하는 것을 막는 라이선스 충돌은 발견하지 못했습니다.** 단, 이 결론은 의존성·포함된 고지·현재 소스의 출처 표기를 점검한 결과입니다. 코드 전체가 독자 작성되었다거나, 이전 AI 작성 과정에서 참고한 모든 출처가 복원되었다는 의미는 아닙니다.

MIT로 바꾸거나 그대로 둔다고 출처가 불명확한 복사 코드의 문제가 해결되지는 않습니다. 다른 코드에 대한 사용 권한과 고지를 확인하는 일은 어떤 프로젝트 라이선스를 선택하든 필요합니다. MPL도 기여자가 자신의 기여물이 독자 작성물이거나 충분한 권리를 가지고 있다고 믿음을 표시하는 조항을 둡니다. [MPL 2.0 §2.5](https://www.mozilla.org/en-US/MPL/2.0/)

## 점검한 자료

- 공개 전 개발용 패키지 `WhisperOverlay-v0.5.0-preview.5-Windows-x64.zip`의 소스와 고지 파일. 원본 ZIP의 SHA-256은 `dea3ee6715a64ab2eb4563c333587938e0ff2dbfa9b43283867df597af04f222`입니다.
- 프로젝트 소스 파일 51개: Rust 36개, JavaScript 모듈 12개, PowerShell 3개. 테스트를 포함해 28,940줄입니다. 이 수치는 파일 목록과 표기 검색 범위이며 모든 줄을 법률·표절 관점에서 수작업 심사했다는 뜻은 아닙니다.
- `Cargo.toml`, `Cargo.lock`, `LICENSE`, `THIRD_PARTY_NOTICES.md`, `third_party_licenses/`, README와 구현·설정·검증 문서.
- 새 빌드를 준비하면서 공식 crates.io에서 확보한 Cargo.lock 고정 버전 21개 아카이브 및 동봉 라이선스 파일 비교 결과.

## 확인된 외부 코드

| 범위 | 확인 결과 | 배포 시 취급 |
|---|---|---|
| Rust 의존성 | 직접 의존성은 `serde`, `serde_json`, `windows-sys`. 전이·대상별 의존성을 포함한 Cargo.lock 외부 패키지는 21개 | 각 라이브러리의 원래 라이선스와 고지를 유지 |
| MIT / Apache 선택 라이브러리 | `serde`, `serde_json`, `windows-sys` 등 대부분 `MIT OR Apache-2.0` | MPL 프로젝트와 함께 사용할 수 있으나 해당 라이브러리를 전부 MPL로 바꿨다고 표시하지 않음 |
| `memchr` | `Unlicense OR MIT` | 동봉한 MIT 및 관련 고지 유지 |
| `unicode-ident` | `(MIT OR Apache-2.0) AND Unicode-3.0` | MIT/Apache 중 하나를 따르는 것에 더해 Unicode 고지 조건도 적용 |
| `zmij` | MIT | 저작권·허가 고지 유지 |
| 기존 Node 중계 코드 | `package.json`에 npm 의존성 없음. Node 내장 모듈 사용. Node 실행 파일은 배포본에 포함되지 않음 | Node를 별도로 재배포하는 경우에는 그 배포물의 조건도 별도 확인 |
| Rust 표준 라이브러리 | `COPYRIGHT-library.html` 동봉. 일반 MIT/Apache 조건과 개별 외부 구성 요소 고지를 포함 | 공식 Rust 1.99.1 배포본의 동일 파일과 바이트 단위 일치를 확인했으며 함께 배포 |
| MinGW / GCC 런타임 | MinGW 및 GCC 배포 패키지의 copyright 파일 동봉 | 컴파일러 자체의 라이선스와 실행 파일에 포함되는 런타임 조건을 구분하고, 적용되는 런타임 고지·예외를 유지 |

### Rust 패키지의 원본 대조

빌드 준비 과정에서 21개 패키지의 정확한 버전 아카이브를 `static.crates.io`에서 확보하고 각각의 SHA-256이 `Cargo.lock`의 체크섬과 일치하는 것을 확인했습니다. 각 아카이브의 `Cargo.toml`에서 선언한 라이선스를 읽고, 앞선 개발용 패키지에 포함된 패키지별 라이선스 파일과 대조했습니다. **라이선스 파일 42개가 모두 해당 원본 아카이브의 파일과 바이트 단위로 일치했습니다.** 선언은 `MIT OR Apache-2.0` 18개, `Unlicense OR MIT` 1개, `(MIT OR Apache-2.0) AND Unicode-3.0` 1개, `MIT` 1개입니다.

이 검증은 원본과 고지의 일치를 확인합니다. 라이브러리 저자들의 전체 저작권 계보를 독립적으로 보증하는 검사는 아닙니다. 기계 판독용 비교 기록은 [crate-provenance.json](provenance/crate-provenance.json)에 있습니다.

### MPL과 다른 라이선스의 관계

MPL은 프로젝트가 사용하는 모든 외부 라이브러리를 MPL로 변경하라고 요구하지 않습니다. 별도 파일의 다른 라이선스 구성 요소와 결합할 수 있으며, 해당 구성 요소의 원래 조건은 계속 지켜야 합니다. [Mozilla MPL FAQ](https://www.mozilla.org/en-US/MPL/2.0/FAQ/)

Mozilla는 MIT·ISC·Apache 2.0·2조항/3조항 BSD를 MPL 2.0과 호환되는 허용적 라이선스의 예로 들고 있습니다. 다만 라이브러리를 별도로 사용하는 경우와 그 코드를 MPL 파일 안으로 복사하는 경우는 구분합니다. 복사한 경우에는 원래 저작권·허가 고지를 함께 보존해야 하며, 가능하면 별도 파일에 두는 것을 안내합니다. [Mozilla 허용적 코드 도입 지침](https://www.mozilla.org/en-US/MPL/2.0/permissive-code-into-mpl/)

Unicode License V3는 사용·수정·배포·판매를 허용하면서 저작권·허가 고지를 코드 또는 관련 문서에 포함하도록 합니다. 따라서 `unicode-ident`의 추가 Unicode 고지를 생략하면 안 됩니다. [Unicode 원문](https://www.unicode.org/license.txt)

### 컴파일러와 런타임

GCC 패키지의 copyright 파일에는 GCC 전체의 GPL과 여러 구성 요소의 라이선스가 함께 나옵니다. 그 파일에서 `GPL` 또는 `LGPL` 문자열을 찾았다는 이유만으로 Sideglow 실행 파일 전체에 GPL이 적용된다고 결론 내릴 수 없습니다.

GCC Runtime Library Exception 3.1은 해당 예외가 적용되는 런타임과 독립 모듈을 적격한 컴파일 과정으로 결합한 대상 코드에 추가 배포 허가를 줍니다. 일반적인 Rust 목적 파일과 표준 MinGW 링크 과정은 이 구분에 따라 확인해야 하며, 컴파일러 수정·GCC 내부 표현을 처리하는 독점 플러그인 사용과 같은 별도 조건을 추측해 적용하지 않습니다. [GNU 런타임 예외와 FAQ](https://www.gnu.org/licenses/gcc-exception-3.1-faq.html)

MinGW 공식 저장소도 빌드 도구 자체에 관한 고지와 결과물의 런타임에 관한 고지를 구분하고, 바이너리 배포에 런타임 고지를 동봉하도록 안내합니다. 기존 고지는 새 빌드에 사용하는 실제 Debian 패키지의 고지와 바이트 단위로 일치했습니다. 대상은 MinGW 12.0.0-5와 GCC 14.2.0-19+27+b1입니다. Rust 표준 라이브러리 고지도 공식 Rust 1.99.1 배포본과 일치했으며 SHA-256은 `5647be074c8edf7339fd863055923a8fc80bc5610a8d4661ec3b767b9d392c27`입니다. 비교 기록은 [runtime-provenance.json](provenance/runtime-provenance.json)에 있습니다. [MinGW 고지 구분](https://github.com/mingw-w64/mingw-w64/blob/master/COPYING.MinGW-w64/COPYING.MinGW-w64.txt)

## 프로젝트 자체 소스의 출처에 대해 확인된 범위

공개 전 preview.5 개발용 패키지 소스의 저작권 표기, SPDX, `copied from`, `adapted from`, `derived from`, Stack Overflow·Gist 참조 등 명시적인 출처 표기를 검색했습니다. 해당 패키지의 프로젝트 소스 51개에서는 이러한 외부 코드 도입 표기를 발견하지 못했습니다. 이번 Sideglow 공개본에서 추가한 MPL 헤더와는 구분한 조사입니다.

**출처 표기가 없다는 사실은 복사 코드가 없다는 증거가 아닙니다.** 현재 배포물에는 파일별 최초 작성·참고 URL·복사 여부를 기록한 개발 이력이나 완전한 Git 기록이 없습니다. 모든 과거 대화와 코드 생성 과정을 복원하지 못했으며, 외부 전체 코드베이스에 대한 코드 유사도 검사도 수행하지 않았습니다.

문서에는 Microsoft Win32, Discord OAuth2/RPC/웹후크, OpenAI API 관련 공식 문서 등 31개의 참고 URL이 남아 있습니다. 이것은 구현 근거로 제시된 문서 목록입니다. 그 목록만으로 특정 구현 줄이 어느 문서나 예제에서 복사되었는지는 알 수 없습니다. API 명세·함수의 사용법을 참고하는 것과 예제 구현을 그대로 복사·수정하는 것은 별도로 취급해야 합니다.

Base64url·CRC32·OAuth PKCE·Win32 메시지 루프처럼 표준 또는 일반적으로 쓰이는 알고리즘과 패턴이 존재한다는 사실만으로 특정 프로젝트의 코드를 복제했다고 판단할 수는 없습니다. 반대로 흔한 패턴이라는 이유만으로 어떤 실제 복사 코드도 저작권 조건에서 제외된다고 단정하지 않았습니다.

## 이번 공개본에 적용한 처리

1. 이번 Sideglow 공개본의 프로젝트 자체 소스와 자체 리소스에 MPL 2.0을 적용합니다. 외부 의존성과 런타임은 각각의 원래 라이선스를 유지합니다.
2. 외부 구성 요소의 라이선스·저작권 파일을 그대로 포함합니다. 이전 프로젝트 자체의 MIT 고지 원문은 이번 공개본의 배포 구성에 포함하지 않으며, 외부 구성 요소의 고지와 구분합니다.
3. 공개 전 개발용 패키지에 프로젝트 자체 MIT 표기가 사용되었던 사실은 개발 이력으로 남깁니다. 이 이력이 이번 공개본 전체에 MIT를 함께 적용한다는 뜻은 아닙니다. 개별 제공된 이전 사본에 관하여 유효하게 부여된 이용 권한은 이번 공개본의 MPL 표기로 소급하여 변경하거나 취소하지 않습니다.
4. 실행 파일과 그 버전에 대응하는 수정 가능한 소스를 함께 제공하고, README에 소스를 얻는 위치를 적습니다. [MPL §3.2](https://www.mozilla.org/en-US/MPL/2.0/)
5. 앞으로 외부 코드를 가져올 때는 파일 경로·원본 URL·버전/커밋·라이선스·수정 여부를 남깁니다. 출처와 허가를 확인할 수 없는 코드는 그대로 편입하지 않고, 확인하거나 공개 명세를 기준으로 다시 구현합니다.

현재 확인된 의존성 및 고지를 근거로 MPL 적용이 곤란하다고 판단할 증거는 없습니다. 그러나 이 점검을 **프로젝트 전체의 독창성 인증, 모든 참고 출처의 복원, 법적 분쟁 가능성 0의 보증**으로 설명해서는 안 됩니다.


MinGW v12.0.0의 런타임 전용 원본 고지를 `third_party_licenses/mingw-w64/COPYING.MinGW-w64-runtime.txt`에 추가했습니다. 기존 고지를 대체하지 않고 함께 보존합니다.


## preview.7 빌드의 외부 고지 재확인 — 2026-11-18 (Asia/Seoul)

이번 빌드의 Cargo.lock 고정 crate 아카이브 21개를 공식 체크섬으로 확인하고, 실제 Rust 1.99.1 및 MinGW/GCC 빌드 도구의 고지를 다시 대조했습니다. 동봉한 외부 고지 46개(크레이트 42개, Rust 표준 라이브러리 1개, MinGW/GCC 3개)가 모두 원본과 바이트 단위로 일치합니다. 새로운 Rust 또는 Node 외부 의존성을 추가하지 않았습니다. 비교 결과와 원본별 해시는 [third-party-notice-verification-preview.7.json](provenance/third-party-notice-verification-preview.7.json)에 있습니다.

이 재확인은 외부 고지와 빌드 입력의 일치에 관한 기록입니다. 위에 적은 과거 작성 이력의 한계는 그대로이며, 프로젝트 자체의 이전 MIT 고지 파일을 이번 배포에 다시 넣지 않습니다.
