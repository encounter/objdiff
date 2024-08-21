import wasmInit, {default_diff_obj_config, run_diff_proto} from '../pkg';
import {DiffObjConfig} from "./main";

self.postMessage({type: 'init'} as OutMessage);
await wasmInit({});
self.postMessage({type: 'ready'} as OutMessage);

type ExtractParam<T> = {
    [K in keyof T]: T[K] extends (arg1: infer U, ...args: any[]) => any ? U & { type: K } : never;
}[keyof T];
type HandlerData = ExtractParam<{
    run_diff: typeof run_diff,
}>;
const handlers: {
    [K in HandlerData['type']]: (data: Omit<HandlerData, 'type'>) => unknown
} = {
    'run_diff': run_diff,
};

function run_diff({left, right, config}: {
    left: Uint8Array | undefined,
    right: Uint8Array | undefined,
    config?: DiffObjConfig
}): Uint8Array {
    const cfg = default_diff_obj_config();
    if (config) {
        for (const key in config) {
            if (key in config) {
                cfg[key] = config[key];
            }
        }
    }
    return run_diff_proto(left, right, cfg);
}

export type InMessage = HandlerData & { messageId: number };

export type OutMessage = ({
    type: 'result',
    result: unknown | null,
    error: unknown | null,
} | {
    type: 'init',
    msg: string
} | {
    type: 'ready',
    msg: string
}) & { messageId: number };

self.onmessage = async (event: MessageEvent<InMessage>) => {
    const data = event.data;
    const handler = handlers[data.type];
    if (handler) {
        try {
            const start = performance.now();
            const result = handler(data);
            const end = performance.now();
            console.debug(`Worker message ${data.messageId} took ${end - start}ms`);
            self.postMessage({
                type: 'result',
                result: result,
                error: null,
                messageId: data.messageId
            });
        } catch (error) {
            self.postMessage({
                type: 'result',
                result: null,
                error: error,
                messageId: data.messageId
            });
        }
    } else {
        self.postMessage({
            type: 'result',
            result: null,
            error: `No handler for ${data.type}`,
            messageId: data.messageId
        });
    }
};
