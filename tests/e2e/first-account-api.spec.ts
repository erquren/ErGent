import {
  test as base,
  expect,
  request as requestFactory,
  type APIRequestContext,
  type APIResponse,
} from "@playwright/test";
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtempSync, rmSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createInterface } from "node:readline";

const root = process.cwd();
const serverBin = resolve(root, "target/debug/ergent-server");
const terminalRunner = resolve(root, "tests/fixtures/controlling-tty.py");
const password = "first-account-e2e-password-only";
const invalidCode = "A".repeat(43);

type StreamName = "terminal" | "stdout" | "stderr";
type Running = {
  child: ChildProcess;
  pid?: number;
  output: Record<StreamName, string>;
  error?: string;
  finished: Promise<number | null>;
};
type Server = Running & { origin: string; api: APIRequestContext };
type Sandbox = {
  database: string;
  start: (terminal?: boolean) => Promise<Server>;
  command: (args: string[], terminal?: boolean) => Running;
  sql: <T>(statement: string, parameters?: unknown[]) => T[];
};

async function unusedPort(): Promise<number> {
  const listener = createServer();
  await new Promise<void>((resolve, reject) => {
    listener.once("error", reject);
    listener.listen(0, "127.0.0.1", resolve);
  });
  const address = listener.address();
  if (!address || typeof address === "string") throw new Error("No test port");
  await new Promise<void>((resolve, reject) =>
    listener.close((error) => (error ? reject(error) : resolve())),
  );
  return address.port;
}

function launch(args: string[], terminal: boolean): Running {
  const child = spawn(
    "python3",
    [
      terminalRunner,
      ...(terminal ? [] : ["--no-terminal"]),
      "--",
      serverBin,
      ...args,
    ],
    { cwd: root, stdio: ["ignore", "pipe", "pipe"] },
  );
  const running: Running = {
    child,
    output: { terminal: "", stdout: "", stderr: "" },
    finished: new Promise((resolve) => child.once("close", resolve)),
  };
  child.on("error", (error) => (running.error = error.message));
  child.stderr!.on(
    "data",
    (data) => (running.output.stderr += data.toString()),
  );
  createInterface({ input: child.stdout! }).on("line", (line) => {
    try {
      const message = JSON.parse(line) as {
        pid?: number;
        stream?: StreamName;
        data?: string;
      };
      if (message.pid) running.pid = message.pid;
      if (message.stream && message.data)
        running.output[message.stream] += message.data;
    } catch {
      running.error = "Invalid output from the test terminal runner";
    }
  });
  return running;
}

async function stop(running: Running) {
  if (running.child.exitCode !== null || running.child.signalCode) return;
  running.child.kill("SIGTERM");
  const stopped = await Promise.race([
    running.finished.then(() => true),
    new Promise<false>((resolve) => setTimeout(() => resolve(false), 5000)),
  ]);
  if (!stopped) {
    if (running.pid) {
      try {
        process.kill(-running.pid, "SIGKILL");
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== "ESRCH") throw error;
      }
    }
    running.child.kill("SIGKILL");
    await running.finished;
  }
}

const test = base.extend<{ sandbox: Sandbox }>({
  sandbox: async ({}, use) => {
    const folder = mkdtempSync(join(tmpdir(), "ergent-first-account-"));
    const database = join(folder, "server.db");
    const children: Running[] = [];
    const clients: APIRequestContext[] = [];
    const command = (args: string[], terminal = true) => {
      const child = launch(["--database", database, ...args], terminal);
      children.push(child);
      return child;
    };
    const sql = <T>(statement: string, parameters: unknown[] = []): T[] => {
      // This only inspects/mutates the test's disposable database, never a user's
      // database. Expiry/deletion scenarios need no production test back door.
      const result = spawnSync(
        "python3",
        [
          "-c",
          "import json,sqlite3,sys; db=sqlite3.connect(sys.argv[1],timeout=10); db.row_factory=sqlite3.Row; rows=db.execute(sys.argv[2],json.loads(sys.argv[3])).fetchall(); db.commit(); print(json.dumps([dict(row) for row in rows])); db.close()",
          database,
          statement,
          JSON.stringify(parameters),
        ],
        { encoding: "utf8" },
      );
      expect(result.status, result.stderr).toBe(0);
      return JSON.parse(result.stdout) as T[];
    };
    try {
      await use({
        database,
        command,
        sql,
        start: async (terminal = true) => {
          const port = await unusedPort();
          const origin = `http://127.0.0.1:${port}`;
          const running = command(
            [
              "run",
              "--listen",
              `127.0.0.1:${port}`,
              "--origin",
              origin,
              "--web",
              resolve(root, "apps/web/dist"),
            ],
            terminal,
          );
          const api = await requestFactory.newContext({ baseURL: origin });
          clients.push(api);
          await expect
            .poll(
              async () => {
                if (running.error) throw new Error(running.error);
                if (running.child.exitCode !== null)
                  throw new Error(
                    `Test server exited: ${running.output.stderr}`,
                  );
                try {
                  return (
                    await api.get("/health/ready", { timeout: 1000 })
                  ).ok();
                } catch {
                  return false;
                }
              },
              { timeout: 15000 },
            )
            .toBe(true);
          return Object.assign(running, { origin, api });
        },
      });
    } finally {
      for (const client of clients) await client.dispose();
      for (const child of children.reverse()) await stop(child);
      rmSync(folder, { recursive: true, force: true });
    }
  },
});

test.skip(
  process.platform === "win32",
  "Controlling-PTY E2E fixture is Unix-only; CI runs on Ubuntu",
);

async function codeFrom(running: Running): Promise<string> {
  let code: string | undefined;
  await expect
    .poll(
      () => {
        code = /Setup code: ([A-Za-z0-9_-]{43})(?:\s|$)/.exec(
          running.output.terminal,
        )?.[1];
        return Boolean(code);
      },
      { timeout: 10000 },
    )
    .toBe(true);
  return code!;
}

async function csrf(server: Server) {
  const response = await server.api.get("/api/v1/auth/csrf");
  expect(response.ok()).toBe(true);
  return (await response.json()).data.csrfToken as string;
}

function body(code: string, username = "first-admin") {
  return {
    username,
    password,
    passwordConfirmation: password,
    setupCode: code,
  };
}

function submit(server: Server, token: string, data: object) {
  return server.api.post("/api/v1/auth/setup", {
    headers: { Origin: server.origin, "X-CSRF-Token": token },
    data,
  });
}

async function expectError(
  response: APIResponse,
  status: number,
  code: string,
) {
  expect(response.status()).toBe(status);
  expect((await response.json()).error.code).toBe(code);
}

async function required(server: Server, value: boolean) {
  const response = await server.api.get("/api/v1/auth/setup");
  expect(response.ok()).toBe(true);
  // Public discovery must never reveal the setup code or credential hash.
  expect(await response.json()).toEqual({ data: { required: value } });
}

async function login(server: Server, username: string) {
  const response = await server.api.post("/api/v1/auth/login", {
    headers: { Origin: server.origin, "X-CSRF-Token": await csrf(server) },
    data: { username, password },
  });
  expect(response.status()).toBe(200);
  expect((await response.json()).data.user.username).toBe(username);
}

test("empty install validates its terminal code and credentials before creating an account", async ({
  sandbox,
}) => {
  const startedAt = Math.floor(Date.now() / 1000);
  const server = await sandbox.start();
  const code = await codeFrom(server);
  await required(server, true);
  const [state] = sandbox.sql<{ code_hash: string; code_expires_at: number }>(
    "SELECT code_hash, code_expires_at FROM first_account_setup",
  );
  expect(state.code_hash).toBe(createHash("sha256").update(code).digest("hex"));
  expect(state.code_expires_at).toBeGreaterThanOrEqual(startedAt + 900);
  expect(state.code_expires_at).toBeLessThanOrEqual(
    Math.floor(Date.now() / 1000) + 900,
  );
  expect((server.output.stdout + server.output.stderr).includes(code)).toBe(
    false,
  );

  const token = await csrf(server);
  await expectError(
    await server.api.post("/api/v1/auth/setup", {
      headers: { Origin: "https://untrusted.example", "X-CSRF-Token": token },
      data: body(code),
    }),
    403,
    "ORIGIN_REJECTED",
  );
  await expectError(
    await submit(server, "invalid-csrf", body(code)),
    403,
    "CSRF_INVALID",
  );
  await expectError(
    await submit(server, token, body(invalidCode)),
    400,
    "SETUP_CODE_INVALID",
  );
  await expectError(
    await submit(server, token, {
      ...body(code),
      passwordConfirmation: `${password}-different`,
    }),
    400,
    "VALIDATION_ERROR",
  );
  expect(
    sandbox.sql<{ count: number }>("SELECT COUNT(*) AS count FROM users")[0]
      .count,
  ).toBe(0);
  await required(server, true);
  const created = await submit(server, token, body(code));
  expect(created.status()).toBe(200);
  expect(await created.json()).toEqual({ data: { created: true } });
  expect((await server.api.get("/api/v1/auth/me")).status()).toBe(401);
  await required(server, false);
  await expectError(
    await submit(server, token, body(code, "second-admin")),
    409,
    "SETUP_CLOSED",
  );
  await login(server, "first-admin");
  expect((server.output.stdout + server.output.stderr).includes(code)).toBe(
    false,
  );
});

test("real browser creates the first account, clears the secrets, and performs an ordinary login", async ({
  sandbox,
  page,
}) => {
  const server = await sandbox.start();
  const code = await codeFrom(server);
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto(server.origin);
  await expect(
    page.getByRole("heading", { name: "创建首个账户" }),
  ).toBeVisible();
  await page.getByLabel("用户名", { exact: true }).fill("browser-admin");
  await page.getByLabel("密码", { exact: true }).fill(password);
  await page.getByLabel("确认密码", { exact: true }).fill(password);
  await page.getByLabel("一次性初始化码", { exact: true }).fill(code);
  await page.getByRole("button", { name: "创建账户", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "进入你的工作台" }),
  ).toBeVisible();
  await expect(page.getByLabel("用户名", { exact: true })).toHaveValue(
    "browser-admin",
  );
  await expect(page.getByLabel("密码", { exact: true })).toHaveValue("");
  expect(
    (await page.request.get(`${server.origin}/api/v1/auth/me`)).status(),
  ).toBe(401);
  await required(server, false);
  await page.getByLabel("密码", { exact: true }).fill(password);
  await page.getByRole("button", { name: "连接工作台", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "所有设备，一个工作台。" }),
  ).toBeVisible();
  expect(errors).toEqual([]);
  expect((server.output.stdout + server.output.stderr).includes(code)).toBe(
    false,
  );
});

test("simultaneous requests across two server processes create exactly one account and cannot reopen", async ({
  sandbox,
}) => {
  const first = await sandbox.start();
  const code = await codeFrom(first);
  // A second process sharing the database must retain the original code, even
  // when it has its own terminal. Admission must serialize in SQLite itself.
  const second = await sandbox.start();
  expect(second.output.terminal.includes("Setup code:")).toBe(false);
  const firstToken = await csrf(first);
  const secondToken = await csrf(second);
  const attempts = await Promise.all([
    submit(first, firstToken, body(code, "racer-one")),
    submit(second, secondToken, body(code, "racer-two")),
  ]);
  expect(attempts.map((response) => response.status()).sort()).toEqual([
    200, 409,
  ]);
  const users = sandbox.sql<{ username: string }>("SELECT username FROM users");
  expect(users).toHaveLength(1);
  expect(
    sandbox.sql(
      "SELECT completed, code_hash, code_expires_at FROM first_account_setup",
    ),
  ).toEqual([{ completed: 1, code_hash: null, code_expires_at: null }]);
  await required(first, false);
  await required(second, false);
  await login(first, users[0].username);
  await stop(first);
  await stop(second);
  const restarted = await sandbox.start();
  await required(restarted, false);
  expect(restarted.output.terminal.includes("Setup code:")).toBe(false);
  await expectError(
    await submit(restarted, await csrf(restarted), body(code, "third-admin")),
    409,
    "SETUP_CLOSED",
  );
  await stop(restarted);

  // Simulate deliberate local account removal. The durable completed marker,
  // not the current user count, is the final source of bootstrap eligibility.
  sandbox.sql("DELETE FROM web_sessions");
  sandbox.sql("DELETE FROM users");
  const emptyAgain = await sandbox.start();
  await required(emptyAgain, false);
  await expectError(
    await submit(emptyAgain, await csrf(emptyAgain), body(code)),
    409,
    "SETUP_CLOSED",
  );
  const rotate = sandbox.command(["setup-code"]);
  expect(await rotate.finished).not.toBe(0);
  expect(rotate.output.terminal.includes("Setup code:")).toBe(false);
});

test("headless startup cannot leak or issue a code; operator rotation and expiry survive restart", async ({
  sandbox,
}) => {
  const headless = await sandbox.start(false);
  await required(headless, true);
  expect(
    sandbox.sql("SELECT code_hash, code_expires_at FROM first_account_setup"),
  ).toEqual([{ code_hash: null, code_expires_at: null }]);
  const denied = sandbox.command(["setup-code"], false);
  expect(await denied.finished).not.toBe(0);
  expect(
    (denied.output.stdout + denied.output.stderr).includes("Setup code:"),
  ).toBe(false);
  const issued = sandbox.command(["setup-code"]);
  expect(await issued.finished).toBe(0);
  const firstCode = await codeFrom(issued);
  expect(
    (issued.output.stdout + issued.output.stderr).includes(firstCode),
  ).toBe(false);
  await stop(headless);

  const restarted = await sandbox.start(false);
  expect(
    sandbox.sql<{ code_hash: string }>(
      "SELECT code_hash FROM first_account_setup",
    )[0].code_hash,
  ).toBe(createHash("sha256").update(firstCode).digest("hex"));
  const deniedRotation = sandbox.command(["setup-code"], false);
  expect(await deniedRotation.finished).not.toBe(0);
  expect(
    sandbox.sql<{ code_hash: string }>(
      "SELECT code_hash FROM first_account_setup",
    )[0].code_hash,
  ).toBe(createHash("sha256").update(firstCode).digest("hex"));
  const token = await csrf(restarted);
  sandbox.sql("UPDATE first_account_setup SET code_expires_at = ?", [
    Math.floor(Date.now() / 1000) - 1,
  ]);
  await expectError(
    await submit(restarted, token, body(firstCode)),
    400,
    "SETUP_CODE_INVALID",
  );
  const rotated = sandbox.command(["setup-code"]);
  expect(await rotated.finished).toBe(0);
  const secondCode = await codeFrom(rotated);
  expect(secondCode === firstCode).toBe(false);
  await expectError(
    await submit(restarted, token, body(firstCode)),
    400,
    "SETUP_CODE_INVALID",
  );
  // Also rotate a still-valid code, not just an already-expired one.
  const rotatedAgain = sandbox.command(["setup-code"]);
  expect(await rotatedAgain.finished).toBe(0);
  const thirdCode = await codeFrom(rotatedAgain);
  expect(thirdCode === secondCode).toBe(false);
  await expectError(
    await submit(restarted, token, body(secondCode)),
    400,
    "SETUP_CODE_INVALID",
  );
  expect((await submit(restarted, token, body(thirdCode))).status()).toBe(200);
  await login(restarted, "first-admin");
});

test("invalid-code throttling is durable and independent of ordinary login state", async ({
  sandbox,
}) => {
  const server = await sandbox.start();
  const code = await codeFrom(server);
  const token = await csrf(server);
  for (let i = 0; i < 4; i++)
    await expectError(
      await submit(server, token, body(invalidCode)),
      400,
      "SETUP_CODE_INVALID",
    );
  const limited = await submit(server, token, body(invalidCode));
  await expectError(limited, 429, "SETUP_THROTTLED");
  expect(Number(limited.headers()["retry-after"])).toBeGreaterThan(0);
  await expectError(
    await submit(server, token, body(code)),
    429,
    "SETUP_THROTTLED",
  );
  expect(
    sandbox.sql<{ count: number }>(
      "SELECT COUNT(*) AS count FROM login_ip_limits",
    )[0].count,
  ).toBe(0);
  await stop(server);
  const restarted = await sandbox.start(false);
  await expectError(
    await submit(restarted, await csrf(restarted), body(code)),
    429,
    "SETUP_THROTTLED",
  );
  expect(
    sandbox.sql<{ count: number }>("SELECT COUNT(*) AS count FROM users")[0]
      .count,
  ).toBe(0);
});

test("CLI admin creation closes setup, and an upgraded existing database stays login-only", async ({
  sandbox,
}) => {
  const admin = spawnSync(
    serverBin,
    [
      "--database",
      sandbox.database,
      "admin-create",
      "--username",
      "legacy-admin",
    ],
    {
      input: password,
      encoding: "utf8",
    },
  );
  expect(admin.status, admin.stderr).toBe(0);
  expect(
    sandbox.sql("SELECT completed, code_hash FROM first_account_setup"),
  ).toEqual([{ completed: 1, code_hash: null }]);
  // Recreate the pre-feature database shape while preserving its genuine
  // Argon2 credential. Startup must recognize the already-populated users table.
  sandbox.sql("DROP TRIGGER users_close_first_account_setup");
  sandbox.sql("DROP TABLE first_account_setup");
  sandbox.sql("DROP TABLE setup_ip_limits");
  const server = await sandbox.start();
  await required(server, false);
  expect(server.output.terminal.includes("Setup code:")).toBe(false);
  await expectError(
    await submit(server, await csrf(server), body(invalidCode)),
    409,
    "SETUP_CLOSED",
  );
  await login(server, "legacy-admin");
});
