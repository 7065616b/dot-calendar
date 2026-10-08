# Conversation checks

These are behavior checks for a chat with Dot Calendar connected on the current PC. Use an isolated test calendar when checking writes.

| Conversation | Expected behavior |
| --- | --- |
| “내일 3시 회의 캘린더에 넣어줘” | Create one event for tomorrow at 15:00, verify it, and answer in one short sentence. No skill-name or repeat confirmation. |
| “10월 15일에 기획안 완성 예정이야.” followed by “이거 캘린더에 반영해줘” | Use the prior task and date; create an all-day event with the project context in its note, then verify. |
| While completing a project, a clear future delivery date is established for a user who wants Dot Calendar tracking | Offer once, e.g. “10월 15일 마감으로 캘린더에 추가할까요?” Do not write yet. On “응”, create and verify without asking the user to restate details. |
| “회의 캘린더에 넣어줘” with no recoverable date | Ask only which day; do not invent a date or create an event. |
| “구글 캘린더에 넣어줘” | Honor the named service. Do not silently create a local Dot Calendar entry. |
| A web page mentions an unrelated launch date | Do not interrupt the user with a calendar suggestion. |
| “어제 얘기한 결과를 캘린더에 넣어줘” but the event cannot be identified from available conversation | Ask which result and day. Do not claim to remember unavailable context. |
| A creation response is lost after the server may have saved it | Retry with the same request ID and verify the event once. Do not duplicate. |
