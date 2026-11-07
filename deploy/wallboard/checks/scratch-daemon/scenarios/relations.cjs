// A real dependency change is an edit of both its cards, whatever else its frame holds. A link touches
// neither task, and a card already Blocked says so before and after, so only what the card's revision
// states of its relations can tell. Writing during an outage gathers a mixed frame deterministically:
// the reconnect's baseline wait holds every change back into one frame, where parallel writes would race.
const { FAR_FUTURE, openArmedDisplay, closeDisplay, expectHealthy, namedBy, sameChange } = require('../display.cjs');

// History groups what a display witnessed within 30 seconds, so one more than that keeps groups apart.
const GROUP_APART = 31_000;

module.exports = async function relations(world) {
  const { report, daemon, server } = world;
  const { expect, delay, waitUntil } = report;
  const project = daemon.project('relations');
  const ids = {};
  ids.awaited = project.task('Already awaited');
  ids.blocked = project.task('Already blocked by that', '--after', ids.awaited);
  ids.awaitedNext = project.task('Awaited next');
  ids.arriving = project.task('Unrelated, comes back during an outage');
  ids.departing = project.task('Unrelated, told a date during an outage');
  ids.first = project.task('Comes first, once it is linked');
  ids.then = project.task('Comes next, once it is linked');
  ids.downstream = project.task('Already waits for both', '--after', ids.first, '--after', ids.then);
  ids.proposedFirst = project.task('Comes first, once proposed');
  ids.proposedThen = project.task('Comes next, once proposed');
  const change = namedBy(ids);
  const arrow = (from, to) => `${from}>${to}`;
  project.date(ids.arriving, FAR_FUTURE);
  const display = await openArmedDisplay(world, project, 'relations');
  const start = await display.read();
  expect('the card about to gain a dependency is already Blocked', start.cards[ids.blocked]?.labels.includes('Blocked'), start.cards[ids.blocked]);

  const edits = (state, ...cards) => cards.every(id => state.cards[id]?.motion === 'edit' && state.cards[id].glow > 0);
  const unmarked = (state, ...cards) => cards.every(id => state.cards[id]?.motion === '' && state.cards[id].glow === 0);
  // What `act` wrote, once a new history entry says the frame applied and the next animation frame
  // has shown its cues.
  const applied = async (what, act) => {
    const before = await display.read();
    await act();
    const found = await waitUntil(what, async () => { const now = await display.read(); return now.history > before.history; }, 45_000, 50);
    if (!found) throw Error(`no change applied: ${what}`);
    await delay(200);
    return { before, after: await display.read() };
  };

  // With no card arriving or departing. The arrow implied by the new link goes from a card that is no
  // end of it, and that card is not edited.
  const direct = await applied('the link to apply', () => project.link(ids.then, ids.first));
  expect('a dependency added between two staying cards edits both, and only them', sameChange(change(direct.after), [], [], ['first', 'then']) && edits(direct.after, ids.first, ids.then), change(direct.after));
  expect('its arrow is drawn, and the arrow it implies goes', direct.after.arrows.includes(arrow(ids.first, ids.then)) && direct.before.arrows.includes(arrow(ids.first, ids.downstream)) && !direct.after.arrows.includes(arrow(ids.first, ids.downstream)), direct.after.arrows.length);
  expect('the card whose implied arrow went is not edited', unmarked(direct.after, ids.downstream), direct.after.cards[ids.downstream]);
  await delay(400); // A cue waits for the audio context to resume.
  const sounded = await display.read();
  expect('that edit sounded once', sounded.played === direct.before.played + 1 && sounded.lastKind === 'edit', { played: sounded.played, kind: sounded.lastKind });
  const groupStarted = direct.after.lastAt;

  // A second relation saying the same thing is the same fact.
  await delay(3_500);
  const beforeRepeat = await display.read();
  project.link(ids.then, ids.first);
  await delay(2_000);
  const afterRepeat = await display.read();
  expect('linking the same dependency again edits nobody', afterRepeat.history === beforeRepeat.history && unmarked(afterRepeat, ids.first, ids.then), { before: beforeRepeat.history, after: afterRepeat.history });

  let relation;
  const proposed = await applied('the proposal to apply', () => { relation = project.propose(ids.proposedThen, ids.proposedFirst); });
  expect('a dependency proposed between two staying cards edits both', sameChange(change(proposed.after), [], [], ['proposedFirst', 'proposedThen']) && edits(proposed.after, ids.proposedFirst, ids.proposedThen), change(proposed.after));
  expect('its arrow is drawn as proposed, and it blocks nothing', proposed.after.arrows.includes(`${arrow(ids.proposedFirst, ids.proposedThen)}?`) && !proposed.after.cards[ids.proposedThen].labels.includes('Blocked'), proposed.after.cards[ids.proposedThen]);
  await delay(3_500);
  const accepted = await applied('the acceptance to apply', () => project.accept(relation));
  expect('accepting it edits both, the card that comes first with nothing else changed', sameChange(change(accepted.after), [], [], ['proposedFirst', 'proposedThen']) && edits(accepted.after, ids.proposedFirst, ids.proposedThen), change(accepted.after));
  expect('its arrow is drawn as accepted, and it blocks', accepted.after.arrows.includes(arrow(ids.proposedFirst, ids.proposedThen)) && accepted.after.cards[ids.proposedThen].labels.includes('Blocked'), accepted.after.cards[ids.proposedThen]);

  // What a write during an outage does, once the reconnect's catch-up has applied it.
  const duringOutage = (what, write) => applied(what, async () => {
    server.backend(false);
    await waitUntil('the display to see the outage', async () => (await display.read()).connection === 'false', 15_000);
    await write();
    server.backend(true);
  });
  const stored = id => { const { updated, status, title, what } = project.show(id); return { updated, status, title, what }; };
  const bothStored = () => JSON.stringify([stored(ids.awaitedNext), stored(ids.blocked)]);

  await delay(Math.max(0, groupStarted + GROUP_APART - Date.now()));
  const storedBefore = bothStored();
  const arrival = await duringOutage('the catch-up to apply a link and an arrival', () => {
    project.link(ids.blocked, ids.awaitedNext);
    project.clearDate(ids.arriving);
  });
  expect('the link touched neither task', bothStored() === storedBefore);
  expect('one frame brings it all', arrival.after.history === arrival.before.history + 1, { before: arrival.before.history, after: arrival.after.history });
  expect('a dependency added in a batch with an unrelated arrival edits both of its cards', sameChange(change(arrival.after), ['arriving'], [], ['awaitedNext', 'blocked']), change(arrival.after));
  expect('both of its cards glow', edits(arrival.after, ids.awaitedNext, ids.blocked), [arrival.after.cards[ids.awaitedNext], arrival.after.cards[ids.blocked]]);
  expect('the arrival has its own cue, and no other card has one', arrival.after.cards[ids.arriving]?.motion === 'arrival' && unmarked(arrival.after, ids.awaited, ids.first, ids.then), arrival.after.cards[ids.arriving]);
  expect('the new arrow is drawn, and the card that was Blocked still says so', arrival.after.arrows.includes(arrow(ids.awaitedNext, ids.blocked)) && arrival.after.cards[ids.blocked].labels.includes('Blocked'), arrival.after.cards[ids.blocked]);
  expect('the catch-up is silent', arrival.after.played === arrival.before.played, { before: arrival.before.played, after: arrival.after.played });

  await delay(Math.max(0, arrival.after.lastAt + GROUP_APART - Date.now()));
  const departure = await duringOutage('the catch-up to apply a retraction and a departure', () => {
    project.unlink(ids.blocked, ids.awaitedNext);
    project.date(ids.departing, FAR_FUTURE);
  });
  expect('the retraction touched neither task', bothStored() === storedBefore);
  expect('one frame brings it all', departure.after.history === departure.before.history + 1, { before: departure.before.history, after: departure.after.history });
  expect('a dependency retracted in a batch with an unrelated departure edits both of its cards', sameChange(change(departure.after), [], ['departing'], ['awaitedNext', 'blocked']), change(departure.after));
  expect('both of its cards glow', edits(departure.after, ids.awaitedNext, ids.blocked), [departure.after.cards[ids.awaitedNext], departure.after.cards[ids.blocked]]);
  expect('the departure has its own cue, and no other card has one', departure.after.cards[ids.departing]?.motion === 'departure' && unmarked(departure.after, ids.awaited, ids.arriving, ids.first, ids.then), departure.after.cards[ids.departing]);
  expect('the arrow is gone, and the card still waits for the other', !departure.after.arrows.includes(arrow(ids.awaitedNext, ids.blocked)) && departure.after.cards[ids.blocked].labels.includes('Blocked'), departure.after.cards[ids.blocked]);

  // Replay plays what history kept, oldest first, each group for four seconds.
  await delay(6_500);
  expect('replay starts', await display.startReplay());
  const replayed = [];
  for (let step = 0; step < 6; step++) {
    await delay(200);
    const now = await display.read();
    if (/^LIVE/.test(now.banner)) break;
    replayed.push({ banner: now.banner, editing: edits(now, ids.awaitedNext, ids.blocked) });
    await delay(3_800);
  }
  report.observe('replay of what this display witnessed', replayed);
  const replayedAs = text => replayed.find(step => step.banner.endsWith(text));
  expect('replay names the batch with the arrival, and both cards glow again', replayedAs('1 added · 2 updated')?.editing, replayed);
  expect('replay names the batch with the departure, and both cards glow again', replayedAs('2 updated · 1 removed')?.editing, replayed);
  await delay(1_000);
  expectHealthy(report, 'after replay', await display.read());
  await closeDisplay(report, display, 'relations');
};
