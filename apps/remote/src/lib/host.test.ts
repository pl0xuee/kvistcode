import { afterEach, describe, expect, it } from "vitest";
import { forgetHost, hw, isHeadless, setHeadless } from "./host";

describe("headless wording", () => {
  afterEach(() => forgetHost());

  it("leaves the desktop wording alone", () => {
    expect(isHeadless()).toBe(false);
    expect(hw("Add one in Kvistcode on your computer.")).toBe("Add one in Kvistcode on your computer.");
  });

  it("words pointers for a server", () => {
    setHeadless(true);
    expect(isHeadless()).toBe(true);
    expect(hw("Add one in Kvistcode on your computer.")).toBe("Add one on the server.");
    expect(hw("Start one from here with New agent, or in Kvistcode on your computer.")).toBe("Start one from here with New agent.");
    expect(hw("Pair a different computer")).toBe("Pair a different server");
  });

  it("is remembered between visits and forgotten on unpair", () => {
    setHeadless(true);
    expect(localStorage.getItem("armada.headless")).toBe("1");
    forgetHost();
    expect(localStorage.getItem("armada.headless")).toBeNull();
    expect(isHeadless()).toBe(false);
  });
});
