// These limits match the server's UTF-8 byte limits, not HTML's UTF-16 length.
export function setupValidationMessage(
  username: string,
  password: string,
  passwordConfirmation: string,
): string {
  const bytes = (value: string) => new TextEncoder().encode(value).length;
  if (
    !username ||
    username.replace(/^\p{White_Space}+|\p{White_Space}+$/gu, "") !==
      username ||
    bytes(username) > 120 ||
    /[\p{Cc}\uD800-\uDFFF]/u.test(username)
  )
    return "用户名须为 1–120 UTF-8 字节，不能含首尾空白或控制字符";
  if (
    bytes(password) < 12 ||
    bytes(password) > 1024 ||
    /[\uD800-\uDFFF]/u.test(password)
  )
    return "密码须为 12–1024 UTF-8 字节";
  if (password !== passwordConfirmation) return "两次输入的密码不一致";
  return "";
}
