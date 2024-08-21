import {defineConfig} from 'tsup';
import fs from 'node:fs/promises';

export default defineConfig({
    entry: ['src/main.ts', 'src/worker.ts'],
    clean: true,
    dts: true,
    format: 'esm',
    sourcemap: true,
    splitting: false,
    target: ['es2022', 'chrome89', 'edge89', 'firefox89', 'safari15', 'node14.8'],
    async onSuccess() {
        await fs.copyFile('pkg/objdiff_core_bg.wasm', 'dist/objdiff_core_bg.wasm');
    }
});
