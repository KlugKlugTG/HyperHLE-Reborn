// Closes issues that are missing an IPA link or a description of the bug, and
// reopens them once an edit adds what was missing. A missing log is optional:
// it is pointed out in a comment but never closes the issue.
//
// Used by .github/workflows/issue_completeness.yml through
// actions/github-script. checkIssue() is pure so it can be tested locally:
//   node .github/scripts/issue-completeness.test.js

const LABEL = 'incomplete report';
const SKIP_LABEL = 'skip auto-close';
const MARKER = '<!-- hyperhle-issue-completeness -->';
// Tags our comment while it asks for a log, so adding one can thank the author.
const LOG_MARKER = '<!-- hyperhle-needs-log -->';
const TELEGRAM_FOOTER = 'Filed via the HyperHLE Telegram fix bot.';
const MAINTAINERS = ['OWNER', 'MEMBER', 'COLLABORATOR'];

// Headings used by the issue forms in .github/ISSUE_TEMPLATE and by the
// Telegram bot (telegram_bot/bot/request.py).
const IPA_HEADING = /ipa link/i;
const LOG_HEADING = /\blog\b/i;
const DESCRIPTION_HEADING =
  /describe the bug|what is the problem|bug \/ crash|describe the feature or issue/i;
// The general form is for emulator features, which have no app to attach.
const FEATURE_HEADING =
  /describe the feature or issue|\b(emulator|launcher|app picker|settings)\s+(bug|crash|issue|feature)\b/i;
const EMULATOR_TITLE =
  /^\[(emulator|feature|ui|android)\]|^\s*(emulator|launcher|app picker|settings)\s+(bug|crash|issue|feature)\b/i;
const GAME_HEADING = /app \/ game name|what is the problem with the app/i;

const URL = /https?:\/\/[^\s<>`"'\]]+/gi;
const STORE_URL = /^https?:\/\/(apps|itunes)\.apple\.com\//i;
const IPA_URL = /\.ipa\b/i;
const TEXT_URL = /\.(txt|log)\b/i;
const CLOUD_SHARE_URL =
  /^https?:\/\/([a-z0-9-]+\.)?(drive\.google\.com|mega\.nz|mediafire\.com|1drv\.ms|onedrive\.live\.com|disk\.yandex\.[a-z]+|pixeldrain\.com|dropbox\.com)\//i;
// Log files dragged into the form, or log lines pasted anywhere.
const LOG_ATTACHMENT =
  /https:\/\/github\.com\/user-attachments\/files\/\d+\/[^\s)]*\.(txt|log)\b/i;
const LOG_TEXT = /(?:touchHLE|HyperHLE)(::|\s+UNOFFICIAL|\s+v\d)/;

function sections(body) {
  const out = [];
  let current = null;
  for (const line of body.split(/\r?\n/)) {
    const heading = /^#{2,4}\s+(.+?)\s*$/.exec(line);
    if (heading) {
      current = { heading: heading[1], text: '' };
      out.push(current);
    } else if (/^\s*---+\s*$/.test(line)) {
      // A rule ends the answer, e.g. before the Telegram bot's footer.
      current = null;
    } else if (current) {
      current.text += line + '\n';
    }
  }
  return out;
}

// What is left of a form answer once placeholders and code fences go.
function answer(text) {
  return text
    .replace(/^\s*(`{3,}|~{3,}).*$/gm, '')
    .replace(/_no response_|_none provided_/gi, '')
    .trim();
}

function hasWords(text, minLetters) {
  return (text.match(/\p{L}/gu) || []).length >= minLetters;
}

// Archive.org IPA names often contain parentheses, so keep them in the URL
// but drop the ones closing a Markdown link.
function urls(text) {
  return (text.match(URL) || []).map((u) => {
    while (u.endsWith(')') && u.split('(').length < u.split(')').length) {
      u = u.slice(0, -1);
    }
    return u;
  });
}

function checkIssue(body, title = '') {
  body = body || '';
  title = title || '';
  const parts = sections(body);
  const find = (re) => parts.filter((s) => re.test(s.heading));
  const ipaParts = find(IPA_HEADING);
  const logParts = find(LOG_HEADING);
  const descParts = find(DESCRIPTION_HEADING);
  const hasGameTemplate = ipaParts.length > 0 || find(GAME_HEADING).length > 0;
  const isFeature =
    find(FEATURE_HEADING).length > 0 ||
    (!hasGameTemplate && EMULATOR_TITLE.test(title));

  let description;
  if (descParts.length > 0) {
    description = descParts.some((s) => hasWords(answer(s.text), 10));
  } else {
    // No form: ignore headings, links, code and the certification boxes.
    const prose = body
      .replace(/```[\s\S]*?```/g, '')
      .replace(URL, '')
      .replace(/^\s*#.*$/gm, '')
      .replace(/^\s*- \[[ xX]\].*$/gm, '');
    description = hasWords(prose, 30);
  }

  const telegramIpa = /IPA file attached via Telegram/i.test(body);
  let ipa;
  if (ipaParts.length > 0) {
    ipa =
      telegramIpa ||
      ipaParts.some((s) =>
        urls(answer(s.text)).some((u) => !STORE_URL.test(u)),
      );
  } else {
    ipa =
      telegramIpa ||
      urls(body).some(
        (u) =>
          IPA_URL.test(u) ||
          (CLOUD_SHARE_URL.test(u) && !TEXT_URL.test(u)),
      );
  }

  let log = LOG_ATTACHMENT.test(body) || LOG_TEXT.test(body);
  if (!log) {
    log = logParts.some((s) => {
      const text = answer(s.text);
      const links = urls(text);
      // A link to the IPA pasted into the log field is not a log.
      if (links.length > 0) return links.some((u) => !IPA_URL.test(u));
      return hasWords(text, 20);
    });
  }

  const missing = [];
  if (!description) missing.push('a description of the bug');
  if (!isFeature && !ipa) missing.push('a direct IPA link');
  return { missing, needsLog: !isFeature && !log };
}

const LOG_NOTE = `it has no log from the run that failed, a log is optional but makes the problem much easier to fix, please **edit this issue** to add one if you can
${LOG_MARKER}`;

function closeComment(missing, needsLog, author) {
  const list = missing.map((m) => `- ${m}`).join('\n');
  const log = needsLog ? `\n\nAlso, ${LOG_NOTE}` : '';
  return `${MARKER}
Hi @${author}, this issue has been closed automatically because it is missing:

${list}

Every bug report or fix request needs a description of the bug and a direct link to the IPA (not an App Store page), please **edit this issue** to add what is missing and it will be reopened automatically${log}`;
}

function logComment(author) {
  return `${MARKER}\nHi @${author}, this issue is staying open, but ${LOG_NOTE}`;
}

function reopenComment(needsLog) {
  const log = needsLog ? `, but ${LOG_NOTE}` : '';
  return `${MARKER}
Thanks! The issue now has everything it needs, so it has been reopened${log}`;
}

async function run({ github, context, core }) {
  const issue = context.payload.issue;
  const { owner, repo } = context.repo;
  const labels = (issue.labels || []).map((l) => l.name);
  const viaTelegram = (issue.body || '').includes(TELEGRAM_FOOTER);

  if (issue.pull_request || labels.includes(SKIP_LABEL)) return;
  if (MAINTAINERS.includes(issue.author_association) && !viaTelegram) {
    core.info('Issue opened by a maintainer, skipping.');
    return;
  }

  const { missing, needsLog } = checkIssue(issue.body, issue.title);
  core.info(`Missing: ${missing.join(', ') || 'nothing'}`);
  core.info(`Log: ${needsLog ? 'missing' : 'present or not needed'}`);
  const params = { owner, repo, issue_number: issue.number };

  const comments = await github.paginate(github.rest.issues.listComments, params);
  const ours = comments.find(
    (c) => c.user.type === 'Bot' && c.body.includes(MARKER),
  );
  // Edits only matter for issues this workflow commented on, so editing an
  // issue filed before it existed never closes it.
  if (context.payload.action === 'edited' && !labels.includes(LABEL) && !ours) {
    return;
  }

  const say = async (body) => {
    if (!ours) {
      await github.rest.issues.createComment({ ...params, body });
    } else if (ours.body !== body) {
      await github.rest.issues.updateComment({
        owner,
        repo,
        comment_id: ours.id,
        body,
      });
    }
  };

  if (missing.length === 0) {
    if (issue.state === 'closed') {
      // Only reopen issues this workflow closed, never ones a maintainer did.
      if (!labels.includes(LABEL)) return;
      await github.rest.issues.update({ ...params, state: 'open' });
      await github.rest.issues.removeLabel({ ...params, name: LABEL });
      await say(reopenComment(needsLog));
    } else if (needsLog) {
      await say(logComment(issue.user.login));
    } else if (ours && ours.body.includes(LOG_MARKER)) {
      await say(`${MARKER}\nThanks for adding the log!`);
    }
    return;
  }

  // A closed issue is either already handled or was closed by a maintainer.
  if (issue.state !== 'open') return;

  await say(closeComment(missing, needsLog, issue.user.login));
  await github.rest.issues.addLabels({ ...params, labels: [LABEL] });
  await github.rest.issues.update({
    ...params,
    state: 'closed',
    state_reason: 'not_planned',
  });
}

module.exports = { checkIssue, run, LABEL, SKIP_LABEL, MARKER };
