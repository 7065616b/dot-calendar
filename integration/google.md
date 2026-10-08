# Google Calendar 연결

Dot Calendar의 Google 동기화는 선택 사항입니다. 처음 실행할 때 네트워크에 접속하거나 Google 계정을 요구하지 않습니다. 연결 전에는 로컬 달력만 사용합니다.

## 준비

1. [Google Cloud Console](https://console.cloud.google.com/apis/library/calendar-json.googleapis.com)에서 프로젝트의 Google Calendar API를 사용 설정합니다.
2. [OAuth 동의 화면과 Desktop app OAuth 클라이언트](https://developers.google.com/identity/protocols/oauth2/native-app)를 준비합니다. 앱 설정에 **클라이언트 ID**를 입력합니다. Desktop client secret이 제공된 경우 함께 입력할 수 있습니다.
3. 동기화할 캘린더 ID를 입력합니다. 기본 캘린더라면 `primary`를 사용합니다.
4. 앱의 **Google 연결**을 누르면 시스템 브라우저에서 Google 로그인과 권한 동의가 열립니다. 앱은 `127.0.0.1`의 임시 포트로 인증 응답을 받습니다. 연결이 성공하면 첫 동기화가 시작됩니다. 이후 상단 **동기화** 버튼으로 다시 요청할 수 있습니다.

앱은 Google Calendar 일정 읽기·쓰기를 위한 `calendar.events`와 색상 팔레트를 읽기 위한 `calendar.readonly` 권한을 요청합니다. [Google 설치형 앱 OAuth 가이드](https://developers.google.com/identity/protocols/oauth2/native-app)는 loopback redirect, PKCE, 새로 고침 토큰 흐름을 설명합니다.

## 동기화 동작

- 연결된 경우에만 사용자가 수동으로 동기화하거나 앱이 약 5분 간격으로 변경 사항을 확인합니다. Google의 `nextSyncToken`을 사용해 이후에는 변경된 일정만 요청합니다. [Google 동기화 가이드](https://developers.google.com/workspace/calendar/api/guides/sync)
- 양쪽의 새 일정과 변경 내용을 교환합니다. 이미 연결된 일정을 양쪽에서 바꾼 경우 덮어쓰지 않고 충돌 건수로 보고합니다. Google 일정 수정·삭제에는 ETag 조건부 요청을 사용합니다. [Google 버전 제어 가이드](https://developers.google.com/workspace/calendar/api/guides/version-resources)
- 한쪽에서 지운 **이미 연결된** 일정은 다른 쪽에서도 지웁니다. 처음 연결할 때 기존 Google 일정을 로컬에 가져오고, 기존 로컬 일정은 Google로 보냅니다. 참석자가 있는 초대 일정, 하루를 넘는 일정, 특수 일정 종류, 복잡한 반복 규칙은 자동으로 가져오거나 수정하지 않습니다.
- 상단 **Google 일정 표시**를 끄면 Google에서 가져온 일정이 날짜 칸과 인쇄에서 숨겨집니다. 설정은 이 PC에 저장되고 다시 켜면 표시됩니다. 이는 표시만 바꾸며 로컬 기록 삭제, Google 원격 삭제, 동기화 중단이나 알림 설정 변경을 하지 않습니다. 닷에서 만든 로컬 일정은 Google에 업로드되어도 계속 보입니다. 날짜 칸의 메모에는 체크박스가 없습니다.
- 연결된 일정을 달력에서 삭제하면 다음 동기화에서 Google에서도 삭제됩니다. 기존 메모 내용을 전부 지운 뒤 저장하는 방법도 삭제에 해당합니다. 반복 일정은 시리즈 전체에 적용됩니다. 로컬의 **마지막 삭제 되돌리기**나 `calendar_restore`가 Google 원격 삭제까지 되돌린다고 보장하지 않습니다.
- Google에서 가져온 일정은 원래 계정과 캘린더에 묶어 기록합니다. 다른 Google 계정으로 바꿔 연결해도 이전 계정에서 가져온 로컬 일정을 새 계정으로 자동 업로드하지 않습니다.
- 날짜만 있는 새 일정은 Google의 하루 종일 일정으로 보냅니다. 시작 시간이 있는 **새 로컬 일정**은 서울 시간으로 1시간 길이로 만듭니다. Google에서 가져온 지원 범위의 일정은 로컬에서 제목·메모 등을 수정해도 기존 종료 시각과 길이를 보존합니다. 시작 날짜·시간을 옮기면 같은 길이만큼 종료 시각도 옮깁니다.
- 기본 반복 규칙(매일·매주·매월·매년)과 팝업 알림 한 개를 동기화합니다. 로컬의 완료 상태는 Google 일정의 비공개 확장 속성으로 저장되며 Google Calendar 화면의 완료 표시로 나타나지는 않습니다. 색상은 Google 이벤트 색상표에서 가장 가까운 색으로 표시합니다. [Google 일정 필드 설명](https://developers.google.com/workspace/calendar/api/v3/reference/events)

Google OAuth 토큰은 현재 Windows 사용자 계정의 DPAPI로 보호해 `google-tokens.dpapi`에 저장합니다. 클라이언트 ID·선택적 Desktop client secret·캘린더 ID 설정은 `google-config.json`에 저장되므로 이 파일을 비밀 저장소로 취급하지 마세요. 설치형 앱 클라이언트는 원래 클라이언트 비밀을 안전하게 숨길 수 없습니다. 동기화 기준 정보는 Google 계정·캘린더마다 별도 `google-sync-<해시>.json`에 저장됩니다. 기본 위치는 `%LOCALAPPDATA%\DotCalendar`이며 `DOT_CALENDAR_DATA_DIR` 환경 변수를 지정하면 그 폴더를 사용합니다. 여러 PC에서 사용할 경우 각 PC에서 앱을 설치하고 같은 Google 계정·캘린더에 각각 연결해야 합니다.

앱에서 연결을 끊으면 이 PC의 보호된 토큰을 지웁니다. 동기화 연결 정보는 같은 계정에 다시 연결할 때 중복 일정을 만들지 않도록 로컬에 남깁니다. Google 계정에 부여한 동의 자체를 취소하려면 Google 계정의 앱 액세스 설정에서도 별도로 취소해야 합니다.

이 저장소에서 실제 Google 계정 로그인과 원격 쓰기는 수행하지 않았습니다. 클라이언트 ID 입력과 사용자 로그인 후 연결 상태 및 동기화 결과를 확인해야 합니다.
