#!/usr/bin/env node

import fs from 'node:fs';

const source = fs.readFileSync('site/src/pages/demo/index.astro', 'utf8');

function requireMatch(pattern, message) {
  if (!pattern.test(source)) throw new Error(message);
}

requireMatch(
  /<select id="media-resolution">[\s\S]*?<option value="source" selected>/,
  'browser demo must preserve source resolution by default',
);
requireMatch(
  /<select id="media-fps">[\s\S]*?<option value="source" selected>/,
  'browser demo must preserve source frame rate by default',
);
requireMatch(
  /<input id="media-audio-q"[^>]*\bvalue="1"/,
  'browser demo must default ALA1 audio to lossless q=1',
);
requireMatch(
  /<span>Encoder effort<\/span>[\s\S]*?<select id="media-preset">/,
  'browser demo must distinguish encoder effort from quantizer quality',
);
requireMatch(
  /Synthetic perspective scene/,
  'the primitive perspective sample must not be presented as real 3D footage',
);

for (const stale of [
  'not hidden expert settings',
  'KNOWN FACTS ONLY',
  'Load fixture',
  '1280×720 max canvas',
]) {
  if (source.includes(stale)) throw new Error(`stale demo wording returned: ${stale}`);
}

console.log('browser demo defaults PASS');
