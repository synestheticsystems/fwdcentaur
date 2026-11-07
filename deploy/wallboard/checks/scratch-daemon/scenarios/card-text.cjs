// Text the renderer reckons narrower than the fonts draw it stays inside its card: a title of wide
// characters wraps at half as many of them, and what still runs past a card's edge, as a title of
// capitals or a long excerpt can, is clipped to the card.
const { openArmedDisplay, closeDisplay } = require('../display.cjs');

// Each card: whether a clip of its own box holds its text, so that nothing past the card shows, and
// which of its texts would run past the card unclipped.
const MEASURE = ids => Object.fromEntries(ids.map(id => {
  const node = document.querySelector(`[data-id="${id}"]`);
  const rect = node.querySelector('.wallboard-card'), card = rect.getBoundingClientRect();
  const clip = node.querySelector('.wallboard-card-clip');
  // A nested svg clips what it holds to its viewport: here the card's own box, from the card's corner.
  const clipped = Boolean(clip) && getComputedStyle(clip).overflow === 'hidden' && clip.x.baseVal.value === 0
    && clip.y.baseVal.value === 0 && clip.width.baseVal.value === rect.width.baseVal.value
    && clip.height.baseVal.value === rect.height.baseVal.value;
  const texts = [...node.querySelectorAll('text')].filter(text => !text.closest('.count')).map(text => {
    const drawn = text.getBoundingClientRect();
    const fits = drawn.left >= card.left - 1 && drawn.right <= card.right + 1 && drawn.top >= card.top - 1 && drawn.bottom <= card.bottom + 1;
    return { kind: text.getAttribute('class'), fits, clipped: clipped && clip.contains(text) };
  });
  return [id, { clipped, texts }];
}));

module.exports = async function cardText(world) {
  const { report, daemon } = world;
  const { expect, observe } = report;
  const project = daemon.project('card-text');
  const ids = {
    wide: project.task('新しい表示の配置を確認してから全てのカードを並べ直す作業を終える'),
    capitals: project.task('REVIEW THE WALLBOARD DEPLOYMENT CHECKLIST WITH THE WHOLE TEAM BEFORE FRIDAY'),
    excerpt: project.taskWithWhat('A task with a long excerpt', 'WIDE CAPITALS IN AN EXCERPT, '.repeat(12)),
  };
  const display = await openArmedDisplay(world, project, 'card-text');
  const cards = await display.page.evaluate(MEASURE, Object.values(ids));
  expect('each card\'s text lies in a clip of the card\'s own box, which hides what runs past it',
    Object.values(cards).every(card => card.clipped && card.texts.every(text => text.clipped)),
    Object.fromEntries(Object.entries(ids).map(([name, id]) => [name, cards[id].clipped])));
  expect('a title of wide characters wraps inside its card, with nothing to clip',
    cards[ids.wide].texts.filter(text => text.kind === 'title').every(text => text.fits), cards[ids.wide].texts);
  observe('the texts the clip cuts short', Object.fromEntries(Object.entries(ids).map(([name, id]) =>
    [name, cards[id].texts.filter(text => !text.fits).map(text => text.kind)])));
  await closeDisplay(report, display, 'card-text');
};
