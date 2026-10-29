// A recorded interactive session, played back line by line: typed input, then the turn it sent.
const PROMPT = /^(\S+ › |\$ )(.*)$/;

/** Marks up one transcript line the way the terminal colors it. */
function line(text) {
  const row = document.createElement("span");
  row.className = "ln";
  const prompt = text.match(PROMPT);
  if (prompt) {
    row.classList.add("in");
    const p = document.createElement("span");
    p.className = "prompt";
    p.textContent = prompt[1];
    row.append(p, document.createElement("span"));
    return { row, typed: prompt[2] };
  }
  if (/^ {2}(turn \d|\(not saved\)|queued:|bye)/.test(text) || /session ".*" · /.test(text)) row.classList.add("meta");
  const was = text.indexOf("   → was ");
  if (text.includes("▶")) row.classList.add("pick");
  if (was >= 0) {
    const tail = document.createElement("span");
    tail.className = "was";
    tail.textContent = text.slice(was);
    row.append(text.slice(0, was), tail);
  } else {
    row.textContent = text;
  }
  return { row, typed: null };
}

/** Colors a static transcript in place, one block per line so long input can wrap. */
export function paint(pre) {
  const lines = pre.textContent.split("\n");
  pre.replaceChildren();
  for (const text of lines) {
    const { row, typed } = line(text);
    if (typed !== null) row.lastChild.textContent = typed;
    row.classList.add("row");
    pre.append(row);
  }
}

const wait = (ms) => new Promise((r) => setTimeout(r, ms));

export function playTranscript(screen, transcript, replay) {
  const lines = transcript.replace(/\n+$/, "").split("\n");
  const still = matchMedia("(prefers-reduced-motion: reduce)").matches;
  let run = 0;

  async function play() {
    const id = ++run;
    screen.replaceChildren();
    for (const text of lines) {
      if (id !== run) return;
      const { row, typed } = line(text);
      screen.append(row, "\n");
      if (typed !== null) {
        const target = row.lastChild;
        if (still) {
          target.textContent = typed;
        } else {
          // Long facts type faster, so every line takes about the same time.
          const step = Math.max(6, Math.min(38, 1100 / Math.max(typed.length, 1)));
          for (const ch of typed) {
            if (id !== run) return;
            target.textContent += ch;
            await wait(step);
          }
          await wait(typed.startsWith("?") || typed === "↵" || typed.startsWith("/whatif") ? 650 : 280);
        }
      } else if (!still) {
        await wait(70);
      }
      screen.scrollTop = screen.scrollHeight;
    }
    replay.hidden = false;
  }

  replay.addEventListener("click", () => {
    replay.hidden = true;
    play();
  });
  if (still) return play();
  replay.hidden = true;
  const seen = new IntersectionObserver((entries) => {
    if (entries.some((e) => e.isIntersecting)) {
      seen.disconnect();
      play();
    }
  }, { threshold: 0.4 });
  seen.observe(screen);
}
