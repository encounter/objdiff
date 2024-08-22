import {defineConfig} from 'tsup';
import fs from 'node:fs/promises';

export default defineConfig([
    // Build main library
    {
        entry: ['src/main.ts'],
        clean: true,
        dts: true,
        format: 'esm',
        outDir: 'dist',
        skipNodeModulesBundle: true,
        sourcemap: true,
        splitting: false,
        target: 'es2022',
    },
    // Build web worker
    {
        entry: ['src/worker.ts'],
        clean: true,
        dts: true,
        format: 'esm', // type: 'module'
        minify: true,
        outDir: 'dist',
        sourcemap: true,
        splitting: false,
        target: 'es2022',
        // https://github.com/egoist/tsup/issues/278
        async onSuccess() {
            await fs.copyFile('pkg/objdiff_core_bg.wasm', 'dist/objdiff_core_bg.wasm');
        }
    }
]);
