# Dot Calendar와 Your dot 연결

바탕화면 달력과 닷은 이 PC의 같은 일정 파일을 사용합니다. 설치 후 닷에게 짧게 말해 일정을 찾거나 바꿀 수 있도록 준비했습니다. 로컬 도구 검사는 통과했으며, 실제 Your dot 계정 대화에서의 호출은 별도 확인이 필요합니다.

## 연결하기

1. [Windows 설치 파일](https://github.com/7065616b/dot-calendar/releases/download/v0.3.12/dot-calendar-0.3.12-windows-x64-setup.exe)을 실행합니다. 현재 사용자 계정에 달력과 닷의 로컬 연결이 함께 설치됩니다.
2. 앱의 **Dot** 버튼에서 다음 문구를 복사해 Your dot에 **한 번** 보냅니다.

   > 내 기본 캘린더는 연결된 PC의 Dot Calendar야. 앞으로 캘린더 요청은 여기에 반영하고, 작업 중 완료 예정일이나 마감일이 나오면 추가할지 먼저 물어봐. 이 설정을 기억하고 오늘 일정을 확인해줘.

3. 이후에는 **“내일 3시 회의 추가해줘”**, **“그 회의를 4시로 옮겨줘”**, **“이거 캘린더에 반영해줘”**처럼 말합니다. 현재 대화에서 작업의 완료 예정일이 나오면 닷이 **“10월 15일 마감으로 캘린더에 추가할까요?”**라고 한 번 묻고, 동의하면 그 일정으로 추가하도록 안내합니다. 사용자가 직접 추가를 요청한 일정은 다시 확인하지 않습니다.

Your dot의 Computers에서 이 PC 접근을 허용하고 PC와 ChatGPT 앱을 온라인 상태로 유지해야 합니다. [공식 컴퓨터 연결 안내](https://learn.chatgpt.com/docs/dots/computers-and-apps)를 참고하세요. 설치 프로그램은 **이 PC의 로컬 연결만** 등록하며, Your dot 계정의 기억이나 기본 달력 선택을 직접 변경하지 않습니다. 문구를 기억하는지와 실제 연결 도구 호출은 사용 환경에서 확인해야 합니다. 다른 달력 서비스를 명시하면 그 선택을 우선합니다.

설치 없는 휴대용 ZIP을 사용한다면 압축을 원하는 고정 폴더에 풀고 앱을 실행한 뒤 **Dot** 버튼에서 이 PC 연결을 등록합니다. 실행 파일 위치를 바꾼 뒤에는 같은 버튼에서 다시 연결합니다.

## 연결 방식과 직접 설치

위젯과 닷 전용 스킬은 같은 Windows 일정 파일을 사용합니다. 스킬은 요청할 때만 Rust MCP 프로세스를 실행하고 종료하며 별도의 상주 서버는 추가하지 않습니다. 명확한 요청은 저장 후 다시 조회해 검증하고, 중복 가능성이 있는 재시도에는 같은 요청 ID를 사용합니다. Google의 실제 원격 반영 여부는 로컬 저장과 별개입니다.

개발 저장소에서는 `dist/dot-calendar.exe`를 빌드한 뒤 PowerShell에서 `./scripts/install-dot-skill.ps1`을 직접 실행할 수도 있습니다. 실행 정책이 차단하는 경우 검토한 스크립트에 한해 `powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\install-dot-skill.ps1`을 사용하고 영구 실행 정책은 바꾸지 않습니다.

설치 위치는 [공식 로컬 스킬 경로](https://learn.chatgpt.com/docs/build-skills)인 `%USERPROFILE%\.agents\skills\dot-calendar`입니다. `-SkillsDirectory`로 다른 호스트 폴더를 지정할 수 있습니다. 설치별 `connection.json`은 실행 파일과 데이터 폴더를 기록합니다. 다른 PC에서는 각각 설치해야 하며 이 파일은 배포 파일에 포함하지 않습니다. 새 대화에서 연결이 보이지 않으면 앱을 재시작한 뒤 다시 시도하세요. 연결된 PC에서 실행 가능한 닷/로컬 대화가 사용하며, ChatGPT 웹·모바일에 이 PC 실행 파일을 직접 등록하지는 않습니다.

## 개발자 명령

PowerShell에서 직접 사용할 수도 있습니다.

    $app = (Resolve-Path .\dist\dot-calendar.exe).Path
    & $app list --date 2026-10-08
    & $app add --date 2026-10-08 --time 15:00 --title '회의' --request-id 'meeting-2026-10-08' --recurrence weekly --reminder-minutes 15
    & $app occurrences --from 2026-10-08 --to 2026-11-08
    & $app update --id '<일정 id>' --date 2026-10-08 --time 16:00 --title '회의' --notes '시간 변경'
    & $app details --id '<일정 id>' --completed true --color '#3366CC'
    & $app delete --id '<일정 id>'

list는 저장된 시리즈를 조회합니다. 반복 날짜를 포함하려면 occurrences를 사용하고, 범위는 최대 366일입니다. update는 날짜·시각·제목·메모를 교체하므로 시각·메모 생략 시 비워집니다. 완료·색·반복·알림 설정은 생략하면 유지됩니다. details는 제공한 설정만 바꿉니다. 색·반복·알림을 지우려면 각각 --color none, --recurrence none, --reminder-minutes none을 사용합니다. 반복 시리즈는 원본 일정 ID를 유지하며 편집과 완료는 시리즈 전체에 적용됩니다.

생성 요청의 --request-id는 같은 요청의 재시도에 재사용하세요. 입력이 동일하면 기존 일정이 반환됩니다. 다른 내용에 같은 ID를 쓰거나 삭제한 ID를 재사용하면 오류가 납니다. 명령은 성공 결과를 JSON 표준 출력, 오류를 표준 오류와 0이 아닌 종료 코드로 전달합니다.

백업은 다음처럼 만들 수 있습니다. 복원은 현재 일정을 전체 교체하므로 앱 메뉴에서는 확인과 복원 전 자동 백업을 거칩니다.

    & $app backup-export --file .\calendar-backup.json
    & $app backup-restore --file .\calendar-backup.json

## 로컬 stdio MCP 호스트

호스트가 로컬 실행 파일을 stdio MCP 서버로 실행할 수 있다면 다음 형태로 등록합니다. 설정 파일 위치와 키 이름은 호스트마다 다릅니다.

    {
      "mcpServers": {
        "dot-calendar": {
          "command": "C:\\Calendar\\dot-calendar.exe",
          "args": ["--mcp"]
        }
      }
    }

서버는 다음 7개 도구를 제공합니다.

| 도구 | 기능 |
| --- | --- |
| calendar_today | PC 현지 날짜의 오늘 일정 조회. 반복 일정과 구글 표시 설정을 반영하고, 완료 항목은 기본적으로 제외 |
| calendar_list | 저장된 원본 일정 조회 |
| calendar_occurrences | 범위 내 반복 발생일 조회 |
| calendar_create | request_id를 사용하는 일정 생성 |
| calendar_update | 날짜·시각·제목·메모 및 선택한 세부 설정 변경 |
| calendar_set_details | 완료·색·반복·알림 설정 변경 |
| calendar_delete | 일정 삭제 |

calendar_today는 `{date, events:[{source, event}]}`를 반환합니다. 각 `event`는 발생일로 바꾸지 않은 원본 전체이며, `source`는 `local` 또는 `google`입니다. `include_completed: true`를 보내면 완료 항목도 나옵니다. 달력의 구글 표시를 끄면 가져온 구글 일정은 이 조회에서도 숨겨집니다. calendar_create의 request_id는 필수입니다. calendar_update의 세부 설정을 생략하면 기존 값이 유지되고, 색·반복·알림에 null을 전달하면 해당 값이 지워집니다. 수정·세부 설정·삭제에는 `calendar_today`의 `event` 또는 `calendar_list`에서 읽은 원본 Event 전체를 `expected`로 보내면 동시 변경 시 덮어쓰기 없이 오류가 반환됩니다. 닷 스킬은 이 비교를 사용합니다. calendar_occurrences의 날짜는 발생일로 바뀌므로 expected에는 사용하지 않습니다. 서버는 표준 출력을 MCP 응답에만 사용합니다.

로컬 stdio MCP 설정만으로 ChatGPT 계정에 서버가 연결되지는 않습니다. [OpenAI 사용자 지정 MCP 안내](https://developers.openai.com/api/docs/guides/custom-mcp-server)는 공개 HTTPS 연결 또는 [Secure MCP Tunnel](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels)을 설명합니다. 현재 저장소는 계정 연결이나 터널을 자동 설정하지 않습니다.

Google Calendar를 통해 여러 PC의 일정을 동기화하려면 각 PC에서 별도로 로그인하고 같은 Google 캘린더를 선택합니다. [Google 연결 안내](google.md)를 확인하세요. 실제 Google 로그인과 기기 간 동기화는 아직 검증하지 않았습니다.
