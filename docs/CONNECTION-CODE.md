# 연결 설정을 코드로 전달하기 — 0.5.0-preview.2

Sideglow는 기존 `WHISPER1:` 코드와 `.whisper.json` 파일도 계속 읽습니다. 새로 내보내는 `SIDEGLOW1:` 코드·`.sideglow.json` 파일을 친구에게 전달할 때는 친구도 새 버전을 사용해야 합니다.

친구에게 JSON 파일을 보내지 않고 **연결 코드 한 개를 복사해서 전달**할 수 있습니다. 코드에는 Discord 앱 ID·채널 링크·선택적인 전송 웹후크만 들어 있습니다. 별도 중계 서버나 코드 조회 서버가 필요하지 않습니다.

## 보내는 사람

1. Sideglow의 **설정 → 연결 공유 코드…**를 엽니다. 연결 방식 옆에 있는 버튼입니다.
2. **현재 설정 코드 복사**를 누릅니다. 현재 설정 초안으로 코드를 만들고 클립보드에 복사합니다.
3. 코드를 신뢰하는 친구에게 전달합니다. 친구에게 별도 설정 파일을 다운로드하라고 할 필요가 없습니다.

처음 연결 안내의 웹후크 준비 화면에서도 공유 코드를 만들 수 있습니다. 복사 전에 앱 ID와 채널을 지정해야 합니다. 웹후크가 없으면 수신 전용 설정을 공유합니다. 복사는 설정 저장이나 메시지 전송을 수행하지 않습니다.

## 받는 사람

1. **설정 → 연결 공유 코드…**에서 **코드 붙여넣기**를 누릅니다. 코드 입력란에 직접 붙여 넣어도 됩니다. 처음 연결 안내의 친구 화면에서도 코드를 불러올 수 있습니다.
2. 표시되는 **앱 ID·채널·웹후크 포함 여부**를 확인합니다. 이 미리보기는 코드의 내용을 읽은 결과이며 Discord의 실제 권한이나 웹후크 대상 채널을 확인한 것은 아닙니다.
3. **설정에 채우기**를 누릅니다. 메인 설정에서 불러왔다면 **저장**을 눌러 적용합니다. 취소하면 불러오지 않습니다.
4. 처음 사용하는 PC에서는 **처음 연결 / 로그인…**으로 자신의 Discord 로그인을 승인합니다. 공유 코드가 계정 로그인이나 채널 접근 권한을 대신하지는 않습니다.

불러온 연결은 현재 편집 중인 채널에 적용합니다. 다른 채널을 보관하고 싶다면 먼저 채널 탭을 추가한 뒤 그 탭에서 불러오세요. 기존 탭과 목적지가 겹치거나 보내지 않은 초안을 보호해야 하는 경우에는 기존 설정 검사에 따라 적용을 막습니다.

불러온 채널의 **이 PC에서 공용 번역 실행**은 꺼진 상태로 시작합니다. 앱 ID가 달라지면 모든 탭이 새 앱 ID를 사용하며, 다른 탭의 번역 실행 선택도 OFF로 바뀝니다. 기존 API 키와 번역 언어·모델은 친구의 코드로 덮어쓰지 않습니다. 번역 담당자는 연결 확인 후 직접 번역을 켜면 됩니다.

## 코드에 들어가는 정보

| 포함 | 제외 |
|---|---|
| Discord 애플리케이션 ID | 개인 Discord 로그인 토큰·새로 고침 토큰 |
| Discord 채널 링크 | 번역 API 키·모델·번역 언어 설정 |
| 전송용 웹후크 URL, 지정한 경우 | 중계 접속 키·게임 경로·단축키·화면 모양·UI 언어 |

**연결 코드는 설정을 되돌릴 수 있는 인코딩이며 암호화가 아닙니다.** 웹후크가 포함된 코드를 가진 사람은 그 웹후크로 메시지를 보낼 수 있습니다. 코드 입력란은 기본으로 가리고 **코드 보기**로 확인할 수 있지만, 가림 표시가 코드 자체를 암호화하지는 않습니다.

코드가 유출되었다면 Discord에서 해당 웹후크를 삭제하거나 재발급하고 새 코드를 공유하세요. 코드만 별도로 만료시키는 서버는 없습니다. 받는 사람의 클립보드에서도 일반 텍스트로 취급됩니다.

## 잘못 복사한 경우

`SIDEGLOW1:`로 시작하는 코드 전체를 전달합니다. ASCII 공백·줄바꿈을 넣어 나눈 코드는 허용하며, 형식이나 오타 확인값 검사에 맞지 않으면 거부합니다. 오류 메시지에는 코드나 웹후크 비밀값을 다시 표시하지 않습니다.

버전·크기·인코딩·오타 확인값·필드와 주소 형식을 검사한 뒤 설정에 채웁니다. 오타 확인값은 잘못된 복사를 발견하기 위한 것이며 보낸 사람의 신원이나 코드의 진위를 증명하지 않습니다. 실제 Discord 접속·웹후크 대상 확인은 기존 연결·전송 단계에서 수행합니다.

기존 JSON 설정 파일 가져오기·내보내기와 앱 ID·채널·웹후크 직접 입력도 사용할 수 있습니다. UI 번역용 `locales/*.json`은 이 연결 공유 방식과 별개입니다.

## 자원 사용과 검증 범위

코드 생성·해석과 클립보드 접근은 이 창에서 사용자가 조작할 때만 수행합니다. 코드 공유용 네트워크·새 백그라운드 스레드·클립보드 감시는 추가하지 않습니다. 코드 전달만을 위한 서버나 새 Rust 의존성도 없습니다.

자동 테스트와 빌드 결과, 실제 Windows에서 확인하지 못한 범위는 [VALIDATION.md](VALIDATION.md)에 정리합니다.

## English quick guide

Open **Settings → Connection code…**. Use **Copy current connection code** to generate and copy a code. A friend can choose **Paste code**, review the application ID/channel/webhook presence, and choose **Fill settings**. Save the main settings or continue the explicit Discord sign-in flow to apply the connection.

The code is reversible encoded connection data, not encryption. It may contain a sending webhook credential; share it only with trusted people. It never contains personal Discord tokens or translation API keys. Importing a code does not sign in, send a message, or enable this PC's shared translator. Empty-webhook codes provide receive-only setup.
