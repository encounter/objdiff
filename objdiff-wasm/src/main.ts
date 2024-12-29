import {ArgumentValue, DiffResult, InstructionDiff, RelocationTarget} from "../gen/diff_pb";
import type {
    ArmArchVersion,
    ArmR9Usage,
    DiffObjConfig,
    MipsAbi,
    MipsInstrCategory,
    X86Formatter
} from '../pkg';
import {AnyHandlerData, InMessage, OutMessage} from './worker';

// Export wasm types
export {ArmArchVersion, ArmR9Usage, MipsAbi, MipsInstrCategory, X86Formatter, DiffObjConfig};

// Export protobuf types
export * from '../gen/diff_pb';

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

export async function runDiff(left: Uint8Array | undefined, right: Uint8Array | undefined, diff_config?: DiffObjConfig): Promise<DiffResult> {
    const data = await defer<Uint8Array>({
        type: 'run_diff_proto',
        left,
        right,
        diff_config
    });
    const parseStart = performance.now();
    const result = DiffResult.fromBinary(data, {readUnknownField: false});
    const end = performance.now();
    console.debug(`Parsing message took ${end - parseStart}ms`);
    return result;
}

export type DiffText =
    DiffTextBasic
    | DiffTextBasicColor
    | DiffTextAddress
    | DiffTextLine
    | DiffTextOpcode
    | DiffTextArgument
    | DiffTextSymbol
    | DiffTextBranchDest
    | DiffTextSpacing;

type DiffTextBase = {
    diff_index?: number,
};
export type DiffTextBasic = DiffTextBase & {
    type: 'basic',
    text: string,
};
export type DiffTextBasicColor = DiffTextBase & {
    type: 'basic_color',
    text: string,
    index: number,
};
export type DiffTextAddress = DiffTextBase & {
    type: 'address',
    address: bigint,
};
export type DiffTextLine = DiffTextBase & {
    type: 'line',
    line_number: number,
};
export type DiffTextOpcode = DiffTextBase & {
    type: 'opcode',
    mnemonic: string,
    opcode: number,
};
export type DiffTextArgument = DiffTextBase & {
    type: 'argument',
    value: ArgumentValue,
};
export type DiffTextSymbol = DiffTextBase & {
    type: 'symbol',
    target: RelocationTarget,
};
export type DiffTextBranchDest = DiffTextBase & {
    type: 'branch_dest',
    address: bigint,
};
export type DiffTextSpacing = DiffTextBase & {
    type: 'spacing',
    count: number,
};

// Native JavaScript implementation of objdiff_core::diff::display::display_diff
export function displayDiff(diff: InstructionDiff, baseAddr: bigint, cb: (text: DiffText) => void) {
    const ins = diff.instruction;
    if (!ins) {
        return;
    }
    if (ins.line_number != null) {
        cb({type: 'line', line_number: ins.line_number});
    }
    cb({type: 'address', address: ins.address - baseAddr});
    if (diff.branch_from) {
        cb({type: 'basic_color', text: ' ~> ', index: diff.branch_from.branch_index});
    } else {
        cb({type: 'spacing', count: 4});
    }
    cb({type: 'opcode', mnemonic: ins.mnemonic, opcode: ins.opcode});
    let arg_diff_idx = 0; // non-PlainText argument index
    for (let i = 0; i < ins.arguments.length; i++) {
        if (i === 0) {
            cb({type: 'spacing', count: 1});
        }
        const arg = ins.arguments[i].value;
        let diff_index: number | undefined;
        if (arg.oneofKind !== 'plain_text') {
            diff_index = diff.arg_diff[arg_diff_idx]?.diff_index;
            arg_diff_idx++;
        }
        switch (arg.oneofKind) {
            case "plain_text":
                cb({type: 'basic', text: arg.plain_text, diff_index});
                break;
            case "argument":
                cb({type: 'argument', value: arg.argument, diff_index});
                break;
            case "relocation": {
                const reloc = ins.relocation!;
                cb({type: 'symbol', target: reloc.target!, diff_index});
                break;
            }
            case "branch_dest":
                if (arg.branch_dest < baseAddr) {
                    cb({type: 'basic', text: '<unknown>', diff_index});
                } else {
                    cb({type: 'branch_dest', address: arg.branch_dest - baseAddr, diff_index});
                }
                break;
        }
    }
    if (diff.branch_to) {
        cb({type: 'basic_color', text: ' ~> ', index: diff.branch_to.branch_index});
    }
}
