// Tests for issue-completeness.js. Run with:
//   node --test .github/scripts/issue-completeness.test.js

const test = require('node:test');
const assert = require('node:assert');
const { checkIssue, run, LABEL, MARKER } = require('./issue-completeness.js');

const CERTIFY = `### By submitting this issue, I certify that…

- [x] I have searched existing issues and this is not a duplicate`;

function fixForm({ ipa, bug, log }) {
  return `### App / game name

Red Ball 3

### IPA link(s)

${ipa}

### HyperHLE version tested (latest from Actions)

b4a533f

### Describe the bug / crash

${bug}

### Log file(s)

\`\`\`text
${log}
\`\`\`

${CERTIFY}`;
}

const IPA = 'https://archive.org/download/Over700iOSGames/Red%20Ball%203.ipa';
const BUG = 'The game freezes a few seconds after the first level loads.';
const LOG = `touchHLE UNOFFICIAL v0.2.3 (f0fcd6e-dirty)
touchHLE::window: Using bundled ANGLE for Android OpenGL ES`;
const HYPERHLE_LOG = `HyperHLE UNOFFICIAL v0.2.3 (f0fcd6e-dirty)
HyperHLE::window: Using bundled ANGLE for Android OpenGL ES`;

test('complete fix request passes', () => {
  const body = fixForm({ ipa: IPA, bug: BUG, log: LOG });
  assert.deepStrictEqual(checkIssue(body), { missing: [], needsLog: false });
});

test('complete fix request passes (HyperHLE log)', () => {
  const body = fixForm({ ipa: IPA, bug: BUG, log: HYPERHLE_LOG });
  assert.deepStrictEqual(checkIssue(body), { missing: [], needsLog: false });
});

test('App Store link is not enough', () => {
  const body = fixForm({
    ipa: 'https://apps.apple.com/ph/app/trigger-fist/id531184261',
    bug: BUG,
    log: LOG,
  });
  assert.deepStrictEqual(checkIssue(body).missing, ['a direct IPA link']);
});

test('a missing log is noted but optional', () => {
  for (const log of ['', '...', '_No response_', IPA]) {
    const body = fixForm({ ipa: IPA, bug: BUG, log });
    const result = checkIssue(body);
    assert.deepStrictEqual(result, { missing: [], needsLog: true }, log);
  }
});

test('empty answers are missing', () => {
  const body = fixForm({ ipa: '_No response_', bug: '_No response_', log: '' });
  assert.deepStrictEqual(checkIssue(body).missing, [
    'a description of the bug',
    'a direct IPA link',
  ]);
});

test('IPA link pasted into the log field is not a log', () => {
  const ipa =
    'https://archive.org/download/x/BackStab-(com.gameloft.BackStab)-1.0.0.ipa';
  const body = fixForm({ ipa: `[BackStab](${ipa})`, bug: BUG, log: ipa });
  assert.deepStrictEqual(checkIssue(body), { missing: [], needsLog: true });
});

test('linked or attached logs count', () => {
  for (const log of [
    '[touchHLE_log.txt](https://github.com/user-attachments/files/33634357/touchHLE_log.txt)',
    "Here's the log: https://www.dropbox.com/scl/fi/abc/touchHLE_log.txt?dl=0",
  ]) {
    const body = fixForm({ ipa: IPA, bug: BUG, log });
    assert.strictEqual(checkIssue(body).needsLog, false, log);
  }
});

test('deprecated form with only the checkbox is missing everything', () => {
  const body = `### By submitting this issue, I certify that…

- [x] I understand this form is deprecated and I should use the “Request a fix for a specific game / app” form`;
  assert.strictEqual(checkIssue(body).missing.length, 2);
});

test('old game fix form without an IPA field is missing the IPA', () => {
  const body = `### App / game name

Forlorn

### What is the problem with the app?

The title screen is broken and the buttons are drawn in the wrong place.

### Log output

\`\`\`text
${LOG}
\`\`\`

${CERTIFY}`;
  assert.deepStrictEqual(checkIssue(body).missing, ['a direct IPA link']);
});

test('feature requests only need a description', () => {
  const body = `### Describe the feature or issue

Please add an option to cap the frame rate so games use less battery.

${CERTIFY}`;
  assert.deepStrictEqual(checkIssue(body), { missing: [], needsLog: false });
  assert.deepStrictEqual(
    checkIssue('### Describe the feature or issue\n\n_No response_').missing,
    ['a description of the bug'],
  );
});

test('free-form body is checked as a whole', () => {
  const body = `Payback crashes when the second mission starts, the screen goes white.
${IPA}
touchHLE::frameworks: something went wrong`;
  assert.deepStrictEqual(checkIssue(body).missing, []);
  const gdriveBody = `Payback crashes when the second mission starts, the screen goes white.
https://drive.google.com/file/d/1AbCdEfGhIjKlMnOpQrStUvWxYz/view?usp=sharing
touchHLE::frameworks: something went wrong`;
  assert.deepStrictEqual(checkIssue(gdriveBody).missing, []);
  const emuBody =
    'Opening the Settings panel from the Android home screen flickers when closing.';
  assert.deepStrictEqual(
    checkIssue(emuBody, '[Emulator] Settings animation flicker').missing,
    [],
  );
  assert.strictEqual(checkIssue('').missing.length, 2);
  assert.strictEqual(checkIssue(null).missing.length, 2);
});

test('Telegram bot issues', () => {
  const body = `**App / game:** \`Payback\`

### IPA link(s)
- \`${IPA}\`

### Bug / crash
\`\`\`
${BUG}
\`\`\`

### Log file(s)
<details><summary>log.txt</summary>

\`\`\`
${LOG}
\`\`\`

</details>

---
_Filed via the HyperHLE Telegram fix bot._`;
  assert.deepStrictEqual(checkIssue(body).missing, []);

  const attached = body.replace(
    `- \`${IPA}\``,
    '- `Payback.ipa`: IPA file attached via Telegram (no public link)',
  );
  assert.deepStrictEqual(checkIssue(attached).missing, []);

  const noLog = body.replace(/<details>[\s\S]*<\/details>/, '_none provided_');
  assert.deepStrictEqual(checkIssue(noLog), { missing: [], needsLog: true });
});

// A fake Octokit that records the calls run() makes.
function fakeGithub(comments = []) {
  const calls = [];
  const rec = (name) => async (args) => {
    calls.push([name, args]);
    return { data: {} };
  };
  const rest = {
    issues: {
      listComments: 'listComments',
      createComment: rec('createComment'),
      updateComment: rec('updateComment'),
      addLabels: rec('addLabels'),
      removeLabel: rec('removeLabel'),
      update: rec('update'),
    },
  };
  return { calls, github: { rest, paginate: async () => comments } };
}

function ctx(issue, action = 'opened') {
  return {
    repo: { owner: 'o', repo: 'r' },
    payload: {
      action,
      issue: {
        number: 7,
        state: 'open',
        labels: [],
        user: { login: 'someone' },
        author_association: 'NONE',
        ...issue,
      },
    },
  };
}

const core = { info() {} };

test('run closes an incomplete issue with a comment and label', async () => {
  const { calls, github } = fakeGithub();
  await run({ github, context: ctx({ body: '' }), core });
  assert.deepStrictEqual(
    calls.map((c) => c[0]),
    ['createComment', 'addLabels', 'update'],
  );
  assert.match(calls[0][1].body, /@someone/);
  assert.match(calls[0][1].body, /a direct IPA link/);
  assert.deepStrictEqual(calls[1][1].labels, [LABEL]);
  assert.strictEqual(calls[2][1].state, 'closed');
  assert.strictEqual(calls[2][1].state_reason, 'not_planned');
});

test('run mentions the missing log when closing for something else', async () => {
  const { calls, github } = fakeGithub();
  const body = fixForm({ ipa: '', bug: BUG, log: '' });
  await run({ github, context: ctx({ body }), core });
  assert.match(calls[0][1].body, /a direct IPA link/);
  assert.match(calls[0][1].body, /no log/);
  assert.strictEqual(calls.at(-1)[1].state, 'closed');
});

test('run comments on a missing log without closing', async () => {
  const { calls, github } = fakeGithub();
  const body = fixForm({ ipa: IPA, bug: BUG, log: '' });
  await run({ github, context: ctx({ body }), core });
  assert.deepStrictEqual(
    calls.map((c) => c[0]),
    ['createComment'],
  );
  assert.match(calls[0][1].body, /@someone/);
  assert.match(calls[0][1].body, /staying open/);
  assert.match(calls[0][1].body, /no log/);
});

test('run thanks the author once the log is added', async () => {
  const { calls: first, github: g1 } = fakeGithub();
  const noLog = fixForm({ ipa: IPA, bug: BUG, log: '' });
  await run({ github: g1, context: ctx({ body: noLog }), core });
  const ours = { id: 3, user: { type: 'Bot' }, body: first[0][1].body };

  const { calls, github } = fakeGithub([ours]);
  const body = fixForm({ ipa: IPA, bug: BUG, log: LOG });
  await run({ github, context: ctx({ body }, 'edited'), core });
  assert.deepStrictEqual(
    calls.map((c) => c[0]),
    ['updateComment'],
  );
  assert.strictEqual(calls[0][1].comment_id, 3);
  assert.match(calls[0][1].body, /Thanks for adding the log/);

  // A repeated edit without a log leaves the note as it is.
  const { calls: again, github: g3 } = fakeGithub([ours]);
  await run({ github: g3, context: ctx({ body: noLog }, 'edited'), core });
  assert.deepStrictEqual(again, []);
});

test('run reopens without a log but still asks for one', async () => {
  const ours = { id: 99, user: { type: 'Bot' }, body: `${MARKER}\nclosed` };
  const { calls, github } = fakeGithub([ours]);
  const body = fixForm({ ipa: IPA, bug: BUG, log: '' });
  const issue = { body, state: 'closed', labels: [{ name: LABEL }] };
  await run({ github, context: ctx(issue, 'edited'), core });
  assert.deepStrictEqual(
    calls.map((c) => c[0]),
    ['update', 'removeLabel', 'updateComment'],
  );
  assert.match(calls[2][1].body, /reopened, but it has no log/);
});

test('run leaves complete issues alone', async () => {
  const { calls, github } = fakeGithub();
  const body = fixForm({ ipa: IPA, bug: BUG, log: LOG });
  await run({ github, context: ctx({ body }), core });
  assert.deepStrictEqual(calls, []);
});

test('run reopens an issue it closed once the edit completes it', async () => {
  const ours = { id: 99, user: { type: 'Bot' }, body: `${MARKER}\nclosed` };
  const { calls, github } = fakeGithub([ours]);
  const body = fixForm({ ipa: IPA, bug: BUG, log: LOG });
  const issue = { body, state: 'closed', labels: [{ name: LABEL }] };
  await run({ github, context: ctx(issue, 'edited'), core });
  assert.deepStrictEqual(
    calls.map((c) => c[0]),
    ['update', 'removeLabel', 'updateComment'],
  );
  assert.strictEqual(calls[0][1].state, 'open');
  assert.strictEqual(calls[2][1].comment_id, 99);
});

test('run does not reopen issues a maintainer closed', async () => {
  const { calls, github } = fakeGithub();
  const body = fixForm({ ipa: IPA, bug: BUG, log: LOG });
  await run({ github, context: ctx({ body, state: 'closed' }, 'edited'), core });
  assert.deepStrictEqual(calls, []);
});

test('run ignores edits to issues it never closed', async () => {
  const { calls, github } = fakeGithub();
  await run({ github, context: ctx({ body: '' }, 'edited'), core });
  assert.deepStrictEqual(calls, []);
});

test('run edits its earlier comment instead of posting again', async () => {
  const ours = { id: 5, user: { type: 'Bot' }, body: `${MARKER}\nold` };
  const { calls, github } = fakeGithub([ours]);
  await run({ github, context: ctx({ body: '' }), core });
  assert.strictEqual(calls[0][0], 'updateComment');
  assert.strictEqual(calls[0][1].comment_id, 5);
});

test('run skips maintainers and the skip label, but not the Telegram bot', async () => {
  for (const issue of [
    { body: '', author_association: 'OWNER' },
    { body: '', labels: [{ name: 'skip auto-close' }] },
  ]) {
    const { calls, github } = fakeGithub();
    await run({ github, context: ctx(issue), core });
    assert.deepStrictEqual(calls, []);
  }
  const { calls, github } = fakeGithub();
  const body = '_Filed via the HyperHLE Telegram fix bot._';
  await run({
    github,
    context: ctx({ body, author_association: 'OWNER' }),
    core,
  });
  assert.strictEqual(calls.at(-1)[1].state, 'closed');
});
