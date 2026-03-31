mod model;
mod render;

use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

pub(crate) use model::CommandOutput;
#[cfg(test)]
pub(crate) use model::ExecCall;
pub(crate) use model::ExecCell;
pub(crate) use render::OutputLinesParams;
pub(crate) use render::TOOL_CALL_MAX_LINES;
pub(crate) use render::new_active_exec_command;
pub(crate) use render::output_lines;
pub(crate) use render::spinner;

static EXEC_OUTPUT_DETAILS_EXPANDED: AtomicBool = AtomicBool::new(false);

pub(crate) fn exec_output_details_expanded() -> bool {
    EXEC_OUTPUT_DETAILS_EXPANDED.load(Ordering::Relaxed)
}

pub(crate) fn collapse_exec_output_details() -> bool {
    EXEC_OUTPUT_DETAILS_EXPANDED.swap(false, Ordering::Relaxed)
}

#[cfg(test)]
pub(crate) fn set_exec_output_details_expanded(expanded: bool) -> bool {
    EXEC_OUTPUT_DETAILS_EXPANDED.swap(expanded, Ordering::Relaxed)
}

pub(crate) fn toggle_exec_output_details() -> bool {
    let previous = EXEC_OUTPUT_DETAILS_EXPANDED.fetch_xor(true, Ordering::Relaxed);
    !previous
}

// 编号（如：1）：修改
// 主要修改内容：为 app-server TUI 的 Exec 单元补充全局详情展开/折叠状态与恢复默认折叠态的方法。
// 修改目的：让 app-server 路径下的命令输出也能默认折叠，并支持 Ctrl 和 O 的临时展开查看。
