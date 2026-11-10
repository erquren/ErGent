// Offline crossterm/ratatui-style fixture. No Codex, account or model access.
// Real raw-mode stdin is the only route for terminal replies and user keys.
const fs = require("node:fs");
const [controlPath, auditPath] = process.argv.slice(2);
if (!process.stdin.isTTY) throw Error("The fixture requires a real PTY");
process.stdin.setRawMode(true);
process.stdin.resume();
let input = "",
  pending,
  lastCommand = 0,
  busy = false;
const audit = {
  ready: true,
  completed: 0,
  replies: [],
  unexpected: [],
  keys: [],
  errors: [],
};
function save() {
  fs.writeFileSync(`${auditPath}.tmp`, JSON.stringify(audit));
  fs.renameSync(`${auditPath}.tmp`, auditPath);
}
function draw(label) {
  // A differential update, deliberately never clearing/reprinting the frame.
  process.stdout.write(`\x1b[3;1H\x1b[2K${label}\x1b[4;9H`);
}
process.stdout.write(
  "\x1b[?1049h\x1b[2J\x1b[HCODEX_TUI_FRAME\x1b[2;1HPERSISTENT_CONTEXT\x1b[3;1HREADY\x1b[4;9H",
);
save();
process.stdin.on("data", (chunk) => {
  input += chunk.toString("utf8");
  while (input.length) {
    if (input.startsWith("\x1b")) {
      const reply = /^\x1b\[[? >]*[0-9;]*[A-Za-z]/.exec(input);
      if (!reply) break; // PTY reads may split a reply anywhere.
      input = input.slice(reply[0].length);
      audit.replies.push(reply[0]);
      if (pending && pending.pattern.test(reply[0])) {
        const done = pending.done;
        pending = undefined;
        done();
      } else audit.unexpected.push(reply[0]);
    } else {
      const key = input[0];
      input = input.slice(1);
      audit.keys.push(key);
      if (key === "k") draw("FOLLOW_UP_KEY_ACCEPTED");
      else audit.errors.push(`Unexpected key ${JSON.stringify(key)}`);
    }
    save();
  }
});
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function query(sequence, pattern) {
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      pending = undefined;
      reject(Error(`Timed out waiting for ${JSON.stringify(sequence)}`));
    }, 2500);
    pending = {
      pattern,
      done: () => {
        clearTimeout(timer);
        resolve();
      },
    };
    // Cursor position must describe the PTY screen, not a browser's stale view.
    process.stdout.write(`\x1b[4;9H${sequence}`);
  });
  // A duplicate from a browser is observable instead of being consumed by the
  // next query. Also permit xterm/transport callbacks to drain before advancing.
  await delay(100);
}
setInterval(async () => {
  if (busy) return;
  let command;
  try {
    command = JSON.parse(fs.readFileSync(controlPath, "utf8"));
  } catch {
    return;
  }
  if (command.id <= lastCommand) return;
  lastCommand = command.id;
  busy = true;
  try {
    await query("\x1b[6n", /^\x1b\[4;9R$/);
    await query("\x1b[5n", /^\x1b\[0n$/);
    await query("\x1b[?6n", /^\x1b\[\?4;9(?:;1)?R$/);
    await query("\x1b[c", /^\x1b\[\?[0-9;]+c$/);
    audit.completed = command.id;
    draw(`TURN_${command.id}_COMPLETE`);
  } catch (error) {
    audit.errors.push(error.message);
  } finally {
    busy = false;
    save();
  }
}, 20);
