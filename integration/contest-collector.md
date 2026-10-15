# 공모전 수집 결과를 Dot Calendar에 반영하기

KST 기준 매일 첫 앱 실행에서 새 공개 원문을 읽는 수집기는 v0.3.17부터 포함한다. 기본 상태는 비활성이며, 기존 PowerShell 7 실행 파일과 설치된 runner·공식 helper의 절대 경로를 검토한 `contest-startup.json` 설정이 있어야 실행한다. 기존 08:00 클라우드 자동화와 중복 실행하지 않도록 실행 주체를 확인한다. 외부 AI API·API 키·새 계정 연결은 사용하지 않는다.

앱을 정상 종료한 상태에서, 실제 `calendar.json`과 같은 폴더에 아래 구조의 `contest-startup.json`을 준비한다. `USER`와 모든 실행 경로는 해당 PC에서 확인한 절대 경로로 바꾼다. 기존 JSON이 있으면 내용을 보존하고 충돌을 먼저 확인한다. PowerShell을 새로 설치하거나 helper의 연결 경로를 바꾸는 명령이 아니다.

```json
{
  "schema_version": 1,
  "enabled": true,
  "powershell_path": "C:\\Program Files\\PowerShell\\7\\pwsh.exe",
  "runner_path": "C:\\Users\\USER\\AppData\\Local\\Programs\\DotCalendar\\scripts\\run-contest-startup.ps1",
  "helper_path": "C:\\Users\\USER\\.agents\\skills\\dot-calendar\\scripts\\invoke-calendar.ps1"
}
```

앱을 다시 실행하면 설정을 읽고 helper의 실제 데이터 경로가 앱과 같은지 검사한다. 끄려면 정상 종료 후 `enabled`를 `false`로 바꾸고 다시 실행한다. 출처별 결과와 보류·제외 사유는 아래의 별도 상태 폴더 안 `daily.json`과 `runs/`에 기록한다.

추가 구현은 `contest-startup` feature가 있는 검증 빌드와 명시적인 `contest-startup.json` 설정에서만 작동한다. 설정이 없거나 꺼져 있으면 수집 스레드·프로세스·타이머를 만들지 않는다. 앱 첫 실행과 기존 날짜 변경·시스템 시각 변경·절전 복귀·사용자 명령 이벤트가 시작점이며, 기존 자정 타이머는 필요한 경우 다음 KST 자정도 함께 반영한다. 앱이 꺼져 있거나 절전 중이면 정해진 시각의 실행은 보장하지 않으며 다음 실행·재개에서 확인한다. 같은 앱에서 사용자 이벤트에 의한 재시작은 최대 시간당 한 번이고, 영속 상태가 같은 KST 날짜의 성공한 소스 재조회를 막는다.

수집 작업은 최대 5회 반복·30분의 활성 예산 안에서만 재시도하며 성공한 날을 기다리는 상주 프로세스는 없다. 개별 공개 요청은 15초, 소스 원문 조회는 2분으로 제한한다. 예산 만료 시 새 소스·Apply를 시작하지 않고 진행 중인 검증·원자 저장을 마무리한다. 따라서 저장 검증 종료까지는 예산보다 잠시 늦게 종료할 수 있다. 실패 소스만 5분·15분·1시간·4시간 간격을 적용하며 KST 날짜당 최대 5회다. 다음 재시도 시간이 활성 예산 밖이면 작업을 종료하고 다음 앱 이벤트에 맡긴다. robots 거부는 그날 재시도하지 않으며 성공 또는 새 공모전 0건으로 기록하지 않는다.

링커리어의 자동 조회 범위는 공개 데이터로 확인한 과학/공학 카테고리(ID 35) 최신 20개와 AI 검색 최신 20개의 합집합이다. 추천 배너는 별도로 제외하고 원문 ID로 중복을 제거한다. 과거 공고 전체를 조회했다고 주장하지 않으며 결과에 `scopeComplete: false`와 목록 URL을 남긴다. DACON은 원문에 실제로 있는 같은 대회의 description/schedule/rules를 함께 읽고, 필수 탭 누락·차단은 partial로 남긴다. 목포대 308 게시판의 공개 RSS를 함께 확인하고, AIFactory의 종료 표시가 있는 공고는 제외한다. DAKER는 현재 robots가 공개 목록 API 접근을 막아 자동 수집할 수 없으며 오류로 남긴다. 제한을 우회하지 않는다. 미확인 자격·모호한 접수 단계·연도/시각·공고 간 충돌은 검토 대기이며 자동 일정으로 생성하지 않는다. 게시 계정을 주최자로 추정하거나 포털의 광범위한 대상 태그만으로 참가 자격을 확정하지 않는다.

수동 후보 반영은 공개 공고를 읽고 [입력 예시](contest-collector.example.json)의 JSON을 UTF-8 파일로 저장한 뒤 연결된 프로젝트에서 실행한다. 이 경로는 새 원문을 수집했다는 증거로 사용하지 않는다. source ID는 실제 공고 번호이며, 신청·작품 제출·팀 병합·리더보드·본선은 각각 별도 stage다. 같은 단계의 key에 마감일이나 변경 가능한 제목을 넣지 않는다.

사용자가 특정 대회를 삭제하도록 요청하면 기본 상태 폴더의 `exclusions.json`에 공고 단위 제외 기록을 보존한다. `source`와 `sourceId`가 같으면 제목·날짜·단계 변경에도 생성과 수정을 금지한다. 같은 공식 URL과 명시 회차의 다른 포털 공고도 제외하며, 회차가 불명확하면 검토 대기로 남긴다. 다른 명시 회차는 별개로 대조한다. 제외 파일이 손상되거나 읽을 수 없으면 달력 호출 전에 중단하고 파일을 초기화하지 않는다. 자동화는 연결된 데이터 경로에 대응하는 기본 상태 폴더를 사용해야 한다.

```powershell
# 먼저 미리보기: 일정과 수집 상태 파일을 변경하지 않는다.
pwsh -NoProfile -File 'E:\dot_calender\scripts\collect-contests.ps1' `
  -CandidatesFile 'E:\dot_calender\qa-data\incoming-contests.json' -Mode Preview `
  -ReportFile 'E:\dot_calender\qa-data\contest-preview-unique.json'

# 검증된 후보 반영: 사용자 요청이 허용한 캘린더 추가 범위에서 사용한다.
pwsh -NoProfile -File 'E:\dot_calender\scripts\collect-contests.ps1' `
  -CandidatesFile 'E:\dot_calender\qa-data\incoming-contests.json' -Mode Apply `
  -ReportFile 'E:\dot_calender\qa-data\contest-apply-unique.json'
```

보고서 경로는 실행마다 새 이름을 사용한다. 기존 로그를 덮어쓰지 않는다. `counts.created`, `counts.updated`, `counts.existing`, `counts.pending`, `counts.excluded`, `counts.sourceErrors`와 `operations`를 읽는다. Preview의 `would_create`·`would_update`는 제안이며 저장 완료가 아니다. `offline`·`blocked`·`partial`을 신규 공고 0건이나 전체 성공으로 설명하지 않는다.

입력 필수 항목은 `schemaVersion: 1`, `runId`, 시각대가 포함된 `collectedAt`, `sourceResults`, `candidates`다. candidate는 `source`, `sourceId`, `sourceUrl`, `title`, `organizer`, `officialUrl`(선택), `status`, `eligibility`, `aiEvidence`, `milestones`를 포함한다. 각 milestone은 `stage`, `date`, `time`(미공개 시 null), `endDate`(없으면 null), `confidence`, `conditional`, `evidence`를 포함한다.

- source: linkareer / dacon / mokpo / daker / aifactory.
- sourceResults.status: ok / partial / error / blocked. 출처 실패는 그 사유를 보존한다.
- `scopeComplete: true`는 해당 출처의 추적 대상 목록을 빠짐없이 검증한 경우에만 붙인다. 일부 페이지 조회에는 붙이지 않는다. 공고가 목록에서 보이지 않는 것을 취소·삭제로 단정하지 않는다.
- status: open / closed / cancelled / practice / test.
- eligibility: eligible / unknown / ineligible. 본인 자격이 미확인이면 unknown으로 둔다.
- aiEvidence.kind: explicit(공고 명시) / category(과학·공학 분류) / context(문맥 추론) / none / forbidden. 확인한 문맥을 text에 남기고 AI 활용 허용과 관련 분야 분류를 혼동하지 않는다.
- confidence: confirmed / uncertain. 연도·날짜를 임의 추정하지 않는다. 시각 미공개는 종일 일정이며, 24:00는 다음날 00:00으로 변환하고 제목에 원래 마감일을 남긴다.
- `edition`은 확인된 연도·회차를 선택적으로 기록한다. 마감 연도에서 추정하지 않는다. 서로 다른 포털의 회차가 미확인이면 같은 신청 URL이라도 검토 대기로 보호한다.

선발·참가자격·수상 조건은 제목·메모에 표시한다. 기간은 한 참고 일정과 메모로 기록하며 매일 펼치지 않는다. 현재 앱에 종료일 필드가 없어 기간 참고 일정은 첫날에만 보인다. 일정·규칙의 장소나 시각이 충돌하면 uncertain으로 검토 대기에 둔다.

기존 일정은 원문 URL·공고 번호·단계와 행사명으로 대조한다. 기존 사용자 일정은 수집기가 소유하지 않으므로 자동으로 덮어쓰지 않는다. 수집기 생성 항목도 사용자가 바꾼 날짜·시각·제목·메모와 완료·색상·반복·알림을 보존한다. 취소·공고 소실·사용자 삭제는 검토·억제 상태로 남기며 삭제·복원으로 처리하지 않는다.

재게시로 공고 번호가 바뀌고 게시기관·연도 접두어가 추가돼도, 행사명 핵심과 같은 단계가 기존 일정·삭제 기록에 있으면 중복 가능성으로 보류한다. 이 유사성은 기존 항목의 소유권이나 동일성을 확정하는 근거로 사용하지 않는다. 서로 다른 명시 회차와 별도 신청·작품 제출 단계는 구분한다.

상태는 기본 `%USERPROFILE%\.dot-calendar-collector\<정규화한 data_directory의 SHA256 앞 16자리>`에 저장하며 실제 DotCalendar 폴더와 분리한다. 경로 끝의 `\`를 제거하고 소문자로 바꾼 절대 데이터 경로의 UTF-8 바이트를 해시한다. 명시적인 `-StateDirectory`는 기존 계약대로 사용한다. 입력 예제의 `collectorState`는 이 규칙을 설명하는 메타데이터이며 실행 경로를 선택하지 않는다.

이전 `%LOCALAPPDATA%\DotCalendarCollector`와 사용자 프로필의 AppData·패키지 LocalCache에 `state.json` 또는 `exclusions.json`이 있으면, 기본 경로를 쓰는 Preview와 Apply 모두 검토된 `migration.json`이 준비되기 전 MCP 호출을 차단한다. 데이터 경로가 바뀌어 해시 폴더가 달라져도 다른 이전 폴더를 새 캘린더와 자동 연결하거나 빈 상태를 만들지 않는다. 패키지 탐색은 사용자 프로필 아래 최대 256개 디렉터리의 `LocalCache\Local\DotCalendarCollector` 대상 폴더 메타데이터만 확인한다.

이전 파일의 안전 사본·원본 및 초기 대상 SHA256 일치를 확인한 뒤 새 상태 폴더에 다음 형식의 `migration.json`을 둔다. `dataDirectory`는 연결 설정의 새 절대 경로와 같아야 하고, `legacySources`는 발견한 모든 이전 폴더를 명시한다. 각 `files`는 해당 폴더에 존재하는 `state.json`과 `exclusions.json`의 이름 및 현재 SHA256을 기록한다. 예제의 자리표시는 실제 검증값으로 바꾼다.

~~~json
{
  "schemaVersion": 1,
  "dataDirectory": "C:\\Users\\USER\\.dot-calendar",
  "legacySources": [
    {
      "directory": "C:\\Users\\USER\\AppData\\Local\\Packages\\PACKAGE\\LocalCache\\Local\\DotCalendarCollector\\OLD_HASH",
      "files": [
        { "name": "exclusions.json", "sha256": "VERIFIED_UPPERCASE_SHA256" }
      ]
    }
  ]
}
~~~

이 확인은 파일을 자동 이동하거나 덮어쓰지 않는다. 이전 원본 해시가 바뀌거나 새 필수 문서가 없으면 `migration_required`로 중단한다. 검토된 초기 복사 이후 새 상태·제외 목록은 사용자의 정상 변경을 허용한다. 배타 실행 lock, 호출 전 intent, 고정 request UUID, 최신 Event의 expected 검증으로 재시도 중복과 동시 수정 덮어쓰기를 막는다. 실행은 GUI와 같은 사용자·Windows 세션을 사용한다. 손상된 상태 파일은 초기화하지 않고 중단한다.

앱이 실행 중이 아니어도 공식 MCP helper가 저장할 수 있으며 다음 실행에 반영된다. PC가 꺼지거나 연결이 없으면 부모는 그 실행을 미반영으로 기록하고 다음 연결에 재대조한다. 로컬 명령이 실제 실행되지 않은 상태를 저장 성공으로 안내하면 안 된다.

공개 본문 수집을 수동으로 시험하려면 다음 명령을 사용한다. 이는 원문 묶음만 출력하며 날짜를 자동 확정하거나 달력을 변경하지 않는다. 부모가 본문을 검증해 후보 JSON을 작성하는 경로가 기본이다.

```powershell
pwsh -NoProfile -File 'E:\dot_calender\scripts\collect-contests.ps1' -CollectSources `
  -MaxDetails 5 `
  -ReportFile 'E:\dot_calender\qa-data\public-pages-unique.json'
```

로그인·CAPTCHA·401/403/429·robots 제한은 우회하지 않는다. sourceResults 오류를 보존하며, 공개 요청에 개인 일정이나 자격증명을 보내지 않는다. Linkareer는 추천 공고가 섞여 ID로 대조하고, DACON은 상세의 description/schedule/rules를 함께 확인한다. 목포대 게시글 번호를 ID로 쓰고 jsessionid를 제외한다. DAKER의 빈 SPA 본문은 미확인으로 처리한다.

회귀 검증: `& .\scripts\test-contest-collector.ps1`. 테스트는 작업 폴더의 QA fixture와 모의 calendar만 사용한다. 실제 적용 테스트에는 별도 exe·연결 파일·데이터 폴더를 사용한다. 실사용 원본의 Apply는 사용자가 허용한 추가 범위에서, 최신 안전 사본과 Preview를 확인한 후 실행한다.
