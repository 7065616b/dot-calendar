"use strict";

// The site uses fictional examples only. It never loads or stores calendar data.
const examples = {
  8: [
    { title: "기획안 정리", note: "첫 페이지 다듬기\n프로젝트 회의 전에 팀에 공유하기" },
    { title: "프로젝트 회의", note: "오후 2시\n진행 상황과 다음 작업 확인" },
    { title: "디자인 초안 검토", note: "달력 날짜 칸과 메모 읽기 화면 확인" },
    { title: "메일 보내기", note: "회의 내용을 간단히 정리해 보내기" },
    { title: "하루 기록", note: "오늘 결정한 내용을 내일 메모에 남기기" },
  ],
  10: [{ title: "주말 계획", note: "가보고 싶은 곳을 적어 두기" }],
  15: [{ title: "할 일 정리", note: "이번 주 마무리할 항목 확인" }],
  22: [{ title: "자료 살펴보기", note: "공유받은 자료 읽고 의견 적기" }],
};

const dateGrid = document.getElementById("date-grid");
const dayTitle = document.getElementById("day-panel-title");
const dayCount = document.getElementById("day-count");
const eventList = document.getElementById("day-event-list");
const memoTitle = document.getElementById("memo-title");
const memoText = document.getElementById("memo-text");
const weekdays = ["일", "월", "화", "수", "목", "금", "토"];
let selectedDay = 8;


function selectEvent(index) {

  const events = examples[selectedDay] || [];
  const event = events[index];
  memoTitle.textContent = event ? event.title : "일정이 없습니다";
  memoText.textContent = event ? event.note : "이 날짜에는 기록된 예시 일정이 없습니다.";
  for (const [buttonIndex, button] of [...eventList.querySelectorAll("button")].entries()) {
    button.setAttribute("aria-current", String(buttonIndex === index));
  }
}

function selectDay(day) {
  selectedDay = day;

  const events = examples[day] || [];
  const weekday = weekdays[new Date(2026, 9, day).getDay()];
  dayTitle.textContent = `10월 ${day}일 ${weekday}요일`;
  dayCount.textContent = `전체 일정 ${events.length}개`;
  eventList.replaceChildren();
  if (events.length === 0) {
    const empty = document.createElement("p");
    empty.className = "empty-day";
    empty.textContent = "이 날짜에는 예시 일정이 없습니다. 다른 날짜를 선택해 보세요.";
    eventList.append(empty);
  } else {
    for (const [index, event] of events.entries()) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "list-event";
      button.textContent = event.title;
      button.addEventListener("click", () => selectEvent(index));
      eventList.append(button);
    }
  }
  selectEvent(0);
  for (const cell of dateGrid.children) {
    const active = Number(cell.dataset.day) === day && cell.dataset.month === "10";
    cell.classList.toggle("is-selected", active);
    const dateButton = cell.querySelector(".day-select");
    if (dateButton) dateButton.setAttribute("aria-pressed", String(active));
  }
}

// October 2026 starts on Thursday. Show the preceding September dates too.
for (let slot = 0; slot < 35; slot += 1) {
  const day = slot < 4 ? 27 + slot : slot - 3;
  const outside = slot < 4;
  const cell = document.createElement("div");
  cell.className = `day-cell${outside ? " outside" : ""}`;
  cell.dataset.day = String(day);
  cell.dataset.month = outside ? "9" : "10";

  const dateButton = document.createElement("button");
  dateButton.type = "button";
  dateButton.className = "day-select";
  dateButton.textContent = String(day);
  dateButton.setAttribute("aria-label", `${outside ? "9월" : "10월"} ${day}일 일정 보기`);
  dateButton.addEventListener("click", () => {
    if (!outside) selectDay(day);
  });
  if (outside) dateButton.disabled = true;
  cell.append(dateButton);

  if (!outside && examples[day]) {
    const holder = document.createElement("div");
    holder.className = "day-events";
    const events = examples[day];
    for (const [index, event] of events.slice(0, 2).entries()) {
      const eventButton = document.createElement("button");
      eventButton.type = "button";
      eventButton.className = "day-event";
      eventButton.textContent = event.title;
      eventButton.setAttribute("aria-label", `10월 ${day}일 ${event.title} 전체 메모 보기`);
      eventButton.addEventListener("click", () => {
        selectDay(day);
        selectEvent(index);
      });
      holder.append(eventButton);
    }
    if (events.length > 2) {
      const overflow = document.createElement("button");
      overflow.type = "button";
      overflow.className = "day-overflow";
      overflow.textContent = `+ ${events.length - 2}개 더 보기`;
      overflow.addEventListener("click", () => selectDay(day));
      holder.append(overflow);
    }
    cell.append(holder);
  }
  dateGrid.append(cell);
}
selectDay(8);

const opacityInput = document.getElementById("opacity");
const opacityValue = document.getElementById("opacity-value");
const demo = document.getElementById("calendar-demo");
opacityInput.addEventListener("input", () => {
  const value = Number(opacityInput.value);
  demo.style.setProperty("--demo-alpha", String(value / 100));
  opacityValue.value = `${value}%`;
  opacityInput.setAttribute("aria-valuetext", `${value}%`);
});
