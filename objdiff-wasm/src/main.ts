import {DiffResult} from "../gen/diff_pb";
import type {
    ConfigProperty,
    MappingConfig,
    SymbolMappings,
} from '../pkg';
import {AnyHandlerData, InMessage, OutMessage} from './worker';

// Export wasm types
export {ConfigProperty, MappingConfig, SymbolMappings};

// Export protobuf types
export * from '../gen/diff_pb';

// Export display types
export * from './display';

interface PromiseCallbacks<T> {
    start: number;
    resolve: (value: T | PromiseLike<T>) => void;
    reject: (reason?: string) => void;
}

let workerInit = false;
let workerCallbacks: PromiseCallbacks<Worker>;
const workerReady = new Promise<Worker>((resolve, reject) => {
    workerCallbacks = {start: performance.now(), resolve, reject};
});

export async function initialize(data?: {
    workerUrl?: string | URL,
    wasmUrl?: string | URL, // Relative to worker URL
}): Promise<Worker> {
    if (workerInit) {
        return workerReady;
    }
    workerInit = true;
    let {workerUrl, wasmUrl} = data || {};
    if (!workerUrl) {
        try {
            // Bundlers will convert this into an asset URL
            workerUrl = new URL('./worker.js', import.meta.url);
        } catch (_) {
            workerUrl = 'worker.js';
        }
    }
    if (!wasmUrl) {
        try {
            // Bundlers will convert this into an asset URL
            wasmUrl = new URL('./objdiff_core_bg.wasm', import.meta.url);
        } catch (_) {
            wasmUrl = 'objdiff_core_bg.js';
        }
    }
    const worker = new Worker(workerUrl, {
        name: 'objdiff',
        type: 'module',
    });
    worker.onmessage = onMessage;
    worker.onerror = (event) => {
        console.error("Worker error", event);
        workerCallbacks.reject("Worker failed to initialize, wrong URL?");
    };
    defer<void>({
        type: 'init',
        // URL can't be sent directly
        wasmUrl: wasmUrl.toString(),
    }, worker).then(() => {
        workerCallbacks.resolve(worker);
    }, (e) => {
        workerCallbacks.reject(e);
    });
    return workerReady;
}

let globalMessageId = 0;
const messageCallbacks = new Map<number, PromiseCallbacks<never>>();

function onMessage(event: MessageEvent<OutMessage>) {
    switch (event.data.type) {
        case 'result': {
            const {result, error, messageId} = event.data;
            const callbacks = messageCallbacks.get(messageId);
            if (callbacks) {
                const end = performance.now();
                console.debug(`Message ${messageId} took ${end - callbacks.start}ms`);
                messageCallbacks.delete(messageId);
                if (error != null) {
                    callbacks.reject(error);
                } else {
                    callbacks.resolve(result as never);
                }
            } else {
                console.warn(`Unknown message ID ${messageId}`);
            }
            break;
        }
    }
}

async function defer<T>(message: AnyHandlerData, worker?: Worker): Promise<T> {
    worker = worker || await initialize();
    const messageId = globalMessageId++;
    const promise = new Promise<T>((resolve, reject) => {
        messageCallbacks.set(messageId, {start: performance.now(), resolve, reject});
    });
    worker.postMessage({
        ...message,
        messageId
    } as InMessage);
    return promise;
}

export async function runDiff(
    left: Uint8Array | null | undefined,
    right: Uint8Array | null | undefined,
    properties?: ConfigProperty[],
    mappingConfig?: MappingConfig,
): Promise<DiffResult> {
    const data = await defer<Uint8Array>({
        type: 'run_diff_proto',
        left,
        right,
        properties,
        mappingConfig,
    });
    const parseStart = performance.now();
    const result = DiffResult.fromBinary(data, {readUnknownField: false});
    const end = performance.now();
    console.debug(`Parsing message took ${end - parseStart}ms`);
    return result;
}
