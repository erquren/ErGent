import { t } from "./i18n";
import type { ToolState, ToolStatus } from "../../../packages/protocol-ts";
export const toolStateLabels: Record<ToolState, string> = {
  idle: "已启动 · 等待任务",
  running: "处理中",
  waiting_for_approval: "等待确认",
  completed: "本轮已完成",
  interrupted: "已中断",
  failed: "进程异常退出",
  stopped: "已退出",
};
export function toolLabel(status: ToolStatus) {
  return `${status.provider === "codex" ? "Codex" : status.provider} · ${t(toolStateLabels[status.state])}`;
}
