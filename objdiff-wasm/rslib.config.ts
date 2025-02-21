import { defineConfig } from '@rslib/core';

export default defineConfig({
  source: {
    entry: {
      'wasi-logging': 'lib/wasi-logging.ts',
    },
  },
  lib: [
    {
      format: 'esm',
      syntax: 'es2022',
    },
  ],
  output: {
    target: 'web',
    copy: [{ from: 'pkg' }, { from: '../objdiff-core/config-schema.json' }],
  },
});
