import { expect, test } from "@playwright/test";
import { readFileSync } from "node:fs";
import { normalizeName, nameValidationMessage } from "../../apps/web/src/names";

const cases: { label: string; value: string; expected: string | null }[] =
  JSON.parse(readFileSync("tests/fixtures/names.json", "utf8"));
for (const fixture of cases) {
  test(`Unicode name: ${fixture.label}`, () => {
    if (fixture.expected === null) {
      expect(() => normalizeName(fixture.value)).toThrow();
      expect(nameValidationMessage(fixture.value)).not.toBe("");
    } else {
      expect(normalizeName(fixture.value)).toBe(fixture.expected);
      expect(nameValidationMessage(fixture.value)).toBe("");
    }
  });
}
test("reject unpaired UTF-16 surrogate before JSON submission", () => {
  expect(() => normalizeName("name\ud800")).toThrow();
  expect(() => normalizeName("name\udc00")).toThrow();
});
