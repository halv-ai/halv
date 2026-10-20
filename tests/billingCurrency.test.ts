import assert from "node:assert/strict";
import test from "node:test";
import { formatPrice } from "../src/billingCurrency.ts";

test("formats whole prices without decimals", () => {
  assert.equal(formatPrice(200, "usd", "en-US"), "$2");
  assert.equal(formatPrice(1_000, "brl", "pt-BR"), "R$\u00a010");
});

test("formats fractional prices with two decimals", () => {
  assert.equal(formatPrice(7_990, "brl", "pt-BR"), "R$\u00a079,90");
});
