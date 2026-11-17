# Discord 중계 서버 — 운영자용

이 폴더는 **운영자 한 명이 실행하는 Node.js 서버**입니다. 친구에게는 Windows 채팅창 실행 파일과 그 사람의 접속 키만 전달합니다. 친구의 PC에 봇 토큰이나 웹훅 주소를 넣지 않습니다.

외부 패키지는 사용하지 않습니다. **Node.js 22 이상**이 필요하며 `npm install`은 필요 없습니다. 이 구현은 지정한 **텍스트 채널 하나**만 취급합니다.

## 동작

1. 채팅창에 입력한 문장이 중계 서버에 도착합니다.
2. 중계 서버가 설정에 등록된 이름으로 Discord incoming webhook에 보냅니다.
3. `wait=true` 응답에서 원문 메시지 ID를 얻어 전송을 확인합니다.
4. Gateway에 연결된 별도 봇이 채널의 메시지와 답글을 수신합니다.
5. 답글의 `message_reference.message_id`를 `parent_id`로 전달하므로, 채팅창은 원문 아래에 답글을 묶어 표시할 수 있습니다.

웹훅 발신 이름은 운영자가 정합니다. 사용자의 일반 Discord 계정으로 대신 로그인하거나 메시지를 보내는 구현은 아닙니다. 사용자는 Discord 공식 클라이언트에서 같은 채널을 사용해도 됩니다.

## 1. Discord에서 준비할 것

### 읽기용 봇

[Discord Developer Portal](https://discord.com/developers/applications)에서 자신의 애플리케이션과 봇을 만들고 대상 서버에 추가합니다.

- Bot 설정에서 **Message Content Intent**를 켭니다.
- 대상 채널의 **View Channel / 채널 보기** 권한을 줍니다.
- 대상 채널의 **Read Message History / 메시지 기록 보기** 권한을 줍니다.
- 이 중계의 봇은 읽기를 담당하므로 서버 관리 권한은 필요 없습니다.
- Bot Token은 운영자만 보관합니다.

이 프로그램은 `GUILD_MESSAGES`와 `MESSAGE_CONTENT` 이벤트를 요청합니다. Discord가 해당 앱에 추가 심사를 요구하면 Developer Portal 안내에 따라 접근 권한을 확보해야 합니다.

### 발신용 웹훅

대상 **텍스트 채널**의 연동 설정에서 incoming webhook을 하나 만들고 URL을 보관합니다. 생성에는 채널의 웹훅 관리 권한이 필요합니다. 봇에 웹훅 관리 권한을 줄 필요는 없습니다.

웹훅 URL도 비밀입니다. 채널 링크(`https://discord.com/channels/...`)와 다릅니다. 중계는 시작할 때 웹훅이 설정된 채널을 가리키는지 확인합니다.

### 채널 ID

Discord의 개발자 모드를 켜고 채널 ID를 복사합니다. 또는 채널 링크에서 마지막 채널 숫자를 확인합니다. 일반 텍스트 채널 링크는 `https://discord.com/channels/서버ID/채널ID` 형식입니다.

## 2. 설정 파일과 개인 접속 키

프로젝트 루트에서 실행합니다.

```powershell
Copy-Item .\relay\config.example.json .\relay\config.json
node .\relay\keygen.mjs
```

키 생성 명령은 **32바이트 난수에서 만든 접속 키 하나**를 출력합니다. 이용자마다 한 번씩 생성해 비밀번호 관리자 등에 보관하세요. 같은 키를 여러 이용자에게 주지 않습니다.

`config.json`을 편집합니다.

| 항목 | 값 |
| --- | --- |
| `channelId` | 허용할 Discord 텍스트 채널 ID |
| `botTokenEnv` | 봇 토큰을 넣는 환경 변수 이름. 기본 `DISCORD_BOT_TOKEN` |
| `webhookUrlEnv` | 웹훅 URL을 넣는 환경 변수 이름. 기본 `DISCORD_WEBHOOK_URL` |
| `users[].id` | 운영자가 정하는 고유 식별자. 영문·숫자·`_`·`-` |
| `users[].displayName` | 그 키로 보낸 메시지에 표시할 이름 |
| `users[].accessKeyEnv` | 그 사람의 접속 키를 넣는 환경 변수 이름 |
| `listen.host` | 기본 `127.0.0.1`. 외부에 직접 열 때에는 TLS 설정 필요 |
| `listen.port` | 기본 `8787` |
| `stateDirectory` | 전송 중복 방지 기록 폴더. 설정 파일 기준 상대 경로, 기본 `.state` |

예시는 두 명으로 되어 있습니다. 혼자 시험한다면 `friend` 항목을 제거해도 됩니다. **실제 토큰과 키를 JSON에 적지 않습니다.** 환경 변수 이름만 적습니다.

### Windows에서 비밀값을 화면에 표시하지 않고 실행

PowerShell에서:

```powershell
.\relay\start.ps1 -ConfigPath .\relay\config.json
```

현재 프로세스의 환경 변수에 값이 없으면 봇 토큰, 웹훅 URL, 이용자 접속 키를 차례로 숨김 입력으로 받습니다. 접속 키에는 앞에서 생성하고 보관한 값을 붙여 넣습니다. 이 실행에서 입력한 값은 프로세스 환경에만 두며 스크립트 종료 시 정리합니다. PowerShell의 스크립트 실행 정책이 스크립트를 허용하지 않는 환경에서는 조직/PC의 정책을 변경하는 대신, 사용 중인 비밀값 관리 도구로 환경 변수를 공급하고 아래 Node 명령을 실행할 수 있습니다.

### 이미 환경 변수를 공급한 환경에서 실행

```text
node relay/main.mjs --config relay/config.json
```

서버가 시작되면 대기 주소를 출력하고 Discord에 연결합니다. 메시지 본문·토큰·키·웹훅 URL은 로그에 쓰지 않습니다. `Ctrl+C`로 종료합니다.

## 3. Windows 채팅창에 입력할 값

운영자 본인이 같은 PC에서 시험할 때:

- 중계 주소: `http://127.0.0.1:8787`
- 채널 링크: 대상 Discord 채널 링크
- 접속 키: `users`에서 자신에게 할당한 키

친구에게는 **자기 키 하나와 HTTPS 중계 주소, 허용한 채널 링크**만 알려줍니다. 봇 토큰과 웹훅 URL은 전달하지 않습니다.

이용자 인증은 운영자가 발급한 키를 기준으로 합니다. Discord OAuth 로그인이나 Discord 역할 동기화는 포함하지 않습니다. **해당 채널을 읽도록 허용된 사람에게만 키를 발급**하고, 이용자가 채널에서 제외되면 설정의 해당 이용자를 제거하거나 키를 바꾼 후 중계를 재시작해야 합니다. 키를 가진 사람은 이 중계를 통해 해당 채널의 최근 기록을 읽고 설정된 이름으로 메시지를 보낼 수 있습니다.

## 4. 다른 PC에서 접속하기

HTTP는 `127.0.0.1` 또는 `::1`에만 바인딩하도록 제한합니다. `http://내공인IP:8787` 형태로 공개할 수 없습니다. 원격 접속에는 다음 중 하나를 사용합니다.

### 중계가 직접 HTTPS 제공

공인 인증 기관이 발급한 인증서와 해당 도메인을 준비한 후 `listen`을 바꿉니다.

```json
{
  "host": "0.0.0.0",
  "port": 8787,
  "tls": {
    "keyFile": "certs/privkey.pem",
    "certFile": "certs/fullchain.pem"
  }
}
```

키와 인증서 경로는 `config.json` 폴더 기준입니다. 클라이언트는 예를 들어 `https://chat.example.com:8787`처럼 인증서 이름과 일치하는 주소를 사용합니다. 자체 서명 인증서를 무조건 신뢰하거나 인증서 검증을 끄는 기능은 제공하지 않습니다.

### 기존 HTTPS reverse proxy 사용

중계는 기본 `127.0.0.1:8787`로 유지하고 HTTPS reverse proxy가 그 주소로 전달하게 설정합니다.

- `Authorization`과 `X-Discord-Channel` 헤더를 전달해야 합니다.
- `/v1/events`의 25초 long poll을 허용하도록 업스트림 응답 제한을 **35초 이상**으로 설정합니다.
- 캐싱과 응답 버퍼링을 끕니다.
- 공개 주소는 HTTPS여야 하며 접속 키를 URL·쿼리 문자열에 넣지 않습니다.
- 브라우저용 CORS는 사용하지 않습니다. `Origin` 헤더를 가진 요청은 이 서버가 거부합니다.

네트워크 공개, 방화벽, 인증서 발급 및 갱신은 이 코드가 자동으로 수행하지 않습니다.

## 5. 처리 범위와 운영 제한

| 항목 | 구현된 동작 |
| --- | --- |
| 기록 | 최신 200개 메시지, 시작/재연결 때 REST로 보완 |
| 변경 내역 | 최신 1,000개 이벤트. 너무 오래된 커서는 전체 최근 기록으로 재동기화 |
| 답글 | `parent_id`로 원문 ID 유지. 원문 전체 객체가 없는 답글도 처리 |
| 수정/삭제 | Gateway update/delete/bulk delete 반영 |
| 접속 변화 | 연결 상태 변화가 long poll을 깨워 즉시 전달 |
| 재연결 | Heartbeat/ACK, Resume, 세션 만료 시 새 연결, 지수 재시도 |
| 발신 | 2,000 UTF-16 코드 단위까지. 일반 한글은 한 글자, 일부 이모지는 두 단위 |
| 멘션 | 발신 메시지의 사용자·역할·전체 멘션 알림을 허용하지 않음 |
| 이름 | 접속 키에 연결된 운영자 설정 이름 사용. 클라이언트가 발신 이름을 바꾸지 못함 |
| 전송 속도 | 이용자별 평균 초당 1회, 최대 5회 버스트, 동시 발신 최대 3개 |
| 동시 수신 | 이용자별 long poll 최대 4개 |
| 메시지 크기 | 표시 메시지 하나의 JSON 최대 32KiB. 아주 긴 내용은 생략 표시 |
| 응답 크기 | 최근 기록 200개는 8MiB 미만, 변경 이벤트는 8MiB 이하로 나눠 전달 |
| 파일/이미지 | 첨부 이름을 텍스트로 표시. 원격 파일 다운로드·미리보기 없음 |

Gateway가 보내는 메시지는 대상 채널 ID를 다시 확인합니다. HTML은 해석하지 않으며 Discord의 Embed도 제목·설명·필드 텍스트만 평문으로 전달합니다.

### 중복 전송 방지

클라이언트는 요청마다 고유 `nonce`를 보냅니다. 서버는 이용자별 nonce와 내용 해시, 전송 결과를 `.state/nonce-state.json`에 기록합니다. 같은 nonce/내용의 중복 요청은 같은 원문 ID를 반환하며, 같은 nonce로 다른 내용을 보내면 거부합니다.

- 실제 웹훅 전송 전에 상태를 디스크에 기록합니다.
- 전송 도중 네트워크가 끊기거나 서버가 종료되어 성공 여부가 불확실하면 같은 nonce를 다시 보내도 새 Discord 메시지를 만들지 않습니다.
- Discord가 명확히 거부한 `429`만 대기 후 다시 시도합니다. 전송했을 수도 있는 시간 초과나 `5xx`는 자동 재전송하지 않습니다.
- 기록은 기본 24시간, 최대 10,000건입니다. **한 상태 폴더를 여러 중계 프로세스가 공유하면 안 됩니다.**
- 상태 파일을 지우거나 24시간 이상 지난 요청은 이전 nonce 보호가 유지되지 않습니다. 사용자 앱도 불확실한 전송을 자동 반복하지 않습니다.

`.state`에는 메시지 본문·접속 키·봇 토큰을 저장하지 않습니다. 이용자 식별자와 전송 메타데이터는 포함하므로 운영자만 접근할 수 있는 폴더에 둡니다. 상태 파일이 손상되면 자동 초기화하여 중복 전송 위험을 만드는 대신 시작을 중단합니다.

## 6. 로컬 검증

프로젝트 루트에서:

```text
node --test relay/test/*.test.mjs
```

테스트는 Node 표준 테스트 도구와 로컬 HTTP 서버, 가짜 Discord REST/Gateway 응답을 사용합니다. **Discord에 실제 접속하거나 메시지를 보내지 않습니다.**

검증한 내용은 인증·허용 채널·이름 고정·메시지 길이·전송 제한·nonce 중복/재시작·불확실한 전송 처리·실제 Reply ID 매핑·Webhook `wait=true`·멘션 차단·429 재시도·Gateway Resume·수정/삭제·long poll·응답 크기 분할입니다.

실제 Discord 서버 권한, 운영자 네트워크, 인증서, 친구 PC 연결 및 게임 위 표시/키보드 동작은 별도 실제 환경 확인이 필요합니다. 이 폴더의 테스트 통과가 그 확인을 대신하지 않습니다.

## 관련 공식 문서

- [Discord Webhook](https://docs.discord.com/developers/resources/webhook)
- [Discord Message와 답글 참조](https://docs.discord.com/developers/resources/message)
- [Discord Gateway](https://docs.discord.com/developers/events/gateway)
- [Discord API Rate Limits](https://docs.discord.com/developers/topics/rate-limits)
