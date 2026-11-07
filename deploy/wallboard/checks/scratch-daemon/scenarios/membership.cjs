// A card coming and going changes no other card's relations, so nobody else is edited: not the ends
// of a chain through it, whose direct arrow is drawn only while it is away, and not a subtask of it,
// whose card names it only while it is drawn.
const { FAR_FUTURE, openArmedDisplay, closeDisplay, namedBy, sameChange } = require('../display.cjs');

module.exports = async function membership(world) {
  const { report, daemon } = world;
  const { expect, observe, delay, waitUntil } = report;
  const project = daemon.project('membership');
  const ids = {};
  ids.chainFirst = project.task('First of a chain');
  ids.chainMiddle = project.task('Middle of a chain', '--after', ids.chainFirst);
  ids.chainLast = project.task('Last of a chain', '--after', ids.chainFirst, '--after', ids.chainMiddle);
  ids.parent = project.task('Parent told a date');
  ids.child = project.task('Subtask of the parent told a date', '--under', ids.parent);
  const change = namedBy(ids);
  const display = await openArmedDisplay(world, project, 'membership');
  const motionOf = (id, motion) => async () => { const now = await display.read(); return now.cards[id]?.motion === motion ? now : null; };

  project.date(ids.chainMiddle, FAR_FUTURE);
  const chainLeaves = await waitUntil('the middle to leave', motionOf(ids.chainMiddle, 'departure'), 15_000);
  expect('a chain\'s middle leaving edits neither neighbour, and the direct arrow is drawn', chainLeaves && sameChange(change(chainLeaves), [], ['chainMiddle'], []) && chainLeaves.arrows.includes(`${ids.chainFirst}>${ids.chainLast}`), chainLeaves && change(chainLeaves));
  await delay(6_500);
  project.clearDate(ids.chainMiddle);
  const chainReturns = await waitUntil('the middle to return', motionOf(ids.chainMiddle, 'arrival'), 15_000);
  expect('a chain\'s middle returning edits neither neighbour', chainReturns && sameChange(change(chainReturns), ['chainMiddle'], [], []), chainReturns && change(chainReturns));
  await delay(6_500);
  expect('and the direct arrow goes once the middle is back', !(await display.read()).arrows.includes(`${ids.chainFirst}>${ids.chainLast}`));

  const parentLine = () => display.page.evaluate(id => document.querySelector(`[data-id="${id}"] .wallboard-parent`)?.textContent ?? null, ids.child);
  const lineBefore = await parentLine();
  project.date(ids.parent, FAR_FUTURE);
  const parentLeaves = await waitUntil('the parent to leave', motionOf(ids.parent, 'departure'), 15_000);
  expect('a parent leaving is no edit of its subtask', parentLeaves && sameChange(change(parentLeaves), [], ['parent'], []) && parentLeaves.cards[ids.child]?.glow === 0, parentLeaves && change(parentLeaves));
  await delay(6_500);
  const lineWithout = await parentLine();
  project.clearDate(ids.parent);
  const parentReturns = await waitUntil('the parent to return', motionOf(ids.parent, 'arrival'), 15_000);
  expect('a parent returning is no edit of its subtask', parentReturns && sameChange(change(parentReturns), ['parent'], [], []) && parentReturns.cards[ids.child]?.glow === 0, parentReturns && change(parentReturns));
  await delay(1_000);
  const lineAfter = await parentLine();
  expect('the subtask\'s card names its parent only while the parent is drawn', Boolean(lineBefore) && lineWithout === null && lineAfter === lineBefore, { lineBefore, lineWithout, lineAfter });
  await delay(6_000);

  const replayed = [];
  if (await display.startReplay()) {
    for (let step = 0; step < 6; step++) {
      replayed.push((await display.read()).banner);
      await delay(4_000);
      if (/^LIVE/.test((await display.read()).banner)) break;
    }
  }
  observe('replay of what this display witnessed, newest five groups', replayed);
  await delay(2_000);
  await closeDisplay(report, display, 'membership');
};
