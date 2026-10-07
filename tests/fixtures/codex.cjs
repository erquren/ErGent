// Offline Codex protocol fixture: no model calls, config or account access.
const { spawnSync } = require('node:child_process');
const readline = require('node:readline');
const args = process.argv.slice(2);
const notify = JSON.parse(args[args.indexOf('-c') + 1].slice('notify='.length));
const agent = process.env.ERGENT_AGENT_BIN;
let turn = 0;
function hook(name) {
  const result = spawnSync(agent, ['codex-hook'], {
    input: JSON.stringify({ hook_event_name: name, session_id: 'fixture-session', turn_id: `turn-${turn}`, prompt: 'PRIVATE_PROMPT_SENTINEL' }),
    encoding: 'utf8',
  });
  if (result.status !== 0 || JSON.parse(result.stdout).decision) throw Error('Hook must not control approvals');
}
console.log('CODEX_FIXTURE_READY');
readline.createInterface({input: process.stdin}).on('line', (line) => {
  if (line === 'prompt') { turn++; hook('UserPromptSubmit'); }
  if (line === 'approval') hook('PermissionRequest');
  if (line === 'resume') hook('PostToolUse');
  if (line === 'finish') spawnSync(notify[0], [...notify.slice(1), JSON.stringify({type:'agent-turn-complete', 'thread-id':'fixture-session', 'turn-id':`turn-${turn}`, 'last-assistant-message':'PRIVATE_ANSWER_SENTINEL'})]);
  if (line === 'interrupt') hook('Interrupt');
  if (line === 'quit') process.exit(0);
  console.log(`FIXTURE_ACK_${line}`);
});
