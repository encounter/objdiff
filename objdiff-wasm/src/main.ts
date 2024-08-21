import {ArgumentValue, DiffResult, InstructionDiff, RelocationTarget} from "../gen/diff_pb";
import type {
    ArmArchVersion,
    ArmR9Usage,
    DiffObjConfig as WasmDiffObjConfig,
    MipsAbi,
    MipsInstrCategory,
    X86Formatter
} from '../pkg';
import {InMessage, OutMessage} from './worker';

// Export wasm types
export type DiffObjConfig = Omit<Partial<WasmDiffObjConfig>, 'free'>;
export {ArmArchVersion, ArmR9Usage, MipsAbi, MipsInstrCategory, X86Formatter};

// Export protobuf types
export * from '../gen/diff_pb';

interface PromiseCallbacks {
    start: number;
    resolve: (value: unknown) => void;
    reject: (reason?: unknown) => void;
}

let workerInit = false;
let workerCallbacks: PromiseCallbacks | null = null;
const workerReady = new Promise<Worker>((resolve, reject) => {
    workerCallbacks = {start: performance.now(), resolve, reject};
});

export function initialize(workerUrl?: string | URL) {
    if (workerInit) {
        return;
    }
    workerInit = true;
    const worker = new Worker(workerUrl || 'worker.js', {type: 'module'});
    worker.onmessage = onMessage.bind(null, worker);
    worker.onerror = (error) => {
        console.error("Worker error", error);
        workerCallbacks.reject(error);
    };
}

let globalMessageId = 0;
const messageCallbacks = new Map<number, PromiseCallbacks>();

function onMessage(worker: Worker, event: MessageEvent<OutMessage>) {
    switch (event.data.type) {
        case 'ready':
            workerCallbacks.resolve(worker);
            break;
        case 'result': {
            const {messageId, result} = event.data;
            const callbacks = messageCallbacks.get(messageId);
            if (callbacks) {
                const end = performance.now();
                console.debug(`Message ${messageId} took ${end - callbacks.start}ms`);
                messageCallbacks.delete(messageId);
                callbacks.resolve(result);
            } else {
                console.warn(`Unknown message ID ${messageId}`);
            }
            break;
        }
    }
}

async function defer<T>(message: Omit<InMessage, 'messageId'>): Promise<T> {
    if (!workerInit) {
        throw new Error('Worker not initialized');
    }
    const worker = await workerReady;
    const messageId = globalMessageId++;
    const promise = new Promise<T>((resolve, reject) => {
        messageCallbacks.set(messageId, {start: performance.now(), resolve, reject});
    });
    worker.postMessage({
        ...message,
        messageId
    });
    return promise;
}

export async function runDiff(left: Uint8Array | undefined, right: Uint8Array | undefined, config?: DiffObjConfig): Promise<DiffResult> {
    const data = await defer<Uint8Array>({
        type: 'run_diff',
        left,
        right,
        config
    } as InMessage);
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

// TypeScript workaround for oneof types
export function oneof<T extends { oneofKind: string }>(type: T): T & { oneofKind: string } {
    return type as T & { oneofKind: string };
}

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
    for (let i = 0; i < ins.arguments.length; i++) {
        if (i === 0) {
            cb({type: 'spacing', count: 1});
        }
        const arg = oneof(ins.arguments[i].value);
        const diff_index = diff.arg_diff[i]?.diff_index;
        switch (arg.oneofKind) {
            case "plain_text":
                cb({type: 'basic', text: arg.plain_text, diff_index});
                break;
            case "argument":
                cb({type: 'argument', value: arg.argument, diff_index});
                break;
            case "relocation": {
                const reloc = ins.relocation!;
                cb({type: 'symbol', target: reloc.target, diff_index});
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
