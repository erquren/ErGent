import { t } from "./i18n";
let csrfToken = "";
export async function csrf() {
  const response = await fetch("/api/v1/auth/csrf");
  if (!response.ok) throw new Error("无法连接服务");
  csrfToken = (await response.json()).data.csrfToken;
}
export async function api<T>(
  path: string,
  method = "GET",
  data?: unknown,
  key?: string,
): Promise<T> {
  const response = await fetch(`/api/v1${path}`, {
    method,
    headers: {
      "Content-Type": "application/json",
      ...(method !== "GET" ? { "X-CSRF-Token": csrfToken } : {}),
      ...(key ? { "Idempotency-Key": key } : {}),
    },
    body: data === undefined ? undefined : JSON.stringify(data),
  });
  if (response.status === 204) return undefined as T;
  const result = await response.json();
  if (!response.ok)
    throw new Error(
      result.error?.message ||
        t("请求失败 ({status})", { status: response.status }),
    );
  if (result.data?.csrfToken) csrfToken = result.data.csrfToken;
  return result.data as T;
}
export async function waitOperation(id: string) {
  for (let n = 0; n < 60; n++) {
    const op = await api<{ status: string; result?: { error?: string } }>(
      `/operations/${id}`,
    );
    if (op.status === "succeeded") return;
    if (op.status === "failed") throw new Error(op.result?.error || "操作失败");
    if (op.status === "unknown")
      throw new Error(
        t("结果尚未确认，请检查会话列表。操作 ID：{id}，不要重复提交。", {
          id,
        }),
      );
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  throw new Error(t("操作尚未确认：{id}，请检查会话列表。", { id }));
}
