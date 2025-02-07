import wasmInit, * as exports from '../pkg';

const handlers = {
    init: init,
    run_diff_proto: run_diff_proto,
} as const;
type ExtractData<T> = T extends (arg: infer U) => Promise<unknown> ? U : never;
type HandlerData = {
    [K in keyof typeof handlers]: { type: K } & ExtractData<typeof handlers[K]>;
};

let wasmReady: Promise<void> | null = null;

async function init({wasmUrl}: { wasmUrl?: string }): Promise<void> {
    if (wasmReady != null) {
        throw new Error('Already initialized');
    }
    wasmReady = wasmInit({module_or_path: wasmUrl})
        .then(() => {
        });
    return wasmReady;
}

async function initIfNeeded() {
    if (wasmReady == null) {
        await init({});
    }
    return wasmReady;
}

async function run_diff_proto({left, right, properties, mappingConfig}: {
    left: Uint8Array | null | undefined,
    right: Uint8Array | null | undefined,
    properties?: exports.ConfigProperty[],
    mappingConfig?: exports.MappingConfig,
}): Promise<Uint8Array> {
    const diffConfig = exports.config_from_properties(properties || []);
    const leftObj = left ? exports.parse_object(left, diffConfig) : null;
    const rightObj = right ? exports.parse_object(right, diffConfig) : null;
    return exports.run_diff(leftObj, rightObj, diffConfig, mappingConfig || {});
}

export type AnyHandlerData = HandlerData[keyof HandlerData];
export type InMessage = AnyHandlerData & { messageId: number };

export type OutMessage = {
    type: 'result',
    result: unknown | null,
    error: string | null,
    messageId: number,
};

self.onmessage = (event: MessageEvent<InMessage>) => {
    const data = event.data;
    const messageId = data?.messageId;
    (async () => {
        if (!data) {
            throw new Error('No data');
        }
        const handler = handlers[data.type];
        if (handler) {
            if (data.type !== 'init') {
                await initIfNeeded();
            }
            const start = performance.now();
            const result = await handler(data as never);
            const end = performance.now();
            console.debug(`Worker message ${data.messageId} took ${end - start}ms`);
            let transfer: Transferable[] = [];
            if (result instanceof Uint8Array) {
                console.log("Transferring!", result.byteLength);
                transfer = [result.buffer];
            } else {
                console.log("Didn't transfer", typeof result);
            }
            self.postMessage({
                type: 'result',
                result: result,
                error: null,
                messageId,
            } as OutMessage, {transfer});
        } else {
            throw new Error(`No handler for ${data.type}`);
        }
    })().catch(error => {
        self.postMessage({
            type: 'result',
            result: null,
            error: error.toString(),
            messageId,
        } as OutMessage);
    });
};
