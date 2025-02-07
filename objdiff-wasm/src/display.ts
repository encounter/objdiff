import {ArgumentValue, InstructionDiff, RelocationTarget} from "../gen/diff_pb";

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
