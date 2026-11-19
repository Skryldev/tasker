import "@testing-library/jest-dom/vitest";
import { afterEach, beforeAll, vi } from "vitest";
import { cleanup } from "@testing-library/react";
import i18next from "i18next";
import { initReactI18next } from "react-i18next";
import enCommon from "@/locales/en/common.json";
import enTasks from "@/locales/en/tasks.json";
import enSettings from "@/locales/en/settings.json";
import enErrors from "@/locales/en/errors.json";
import enNotifications from "@/locales/en/notifications.json";
import enGoals from "@/locales/en/goals.json";

// Every component under test reads strings through i18next; bundle the en
// resources inline so unit tests render real text without async loading.
beforeAll(async () => {
  await i18next.use(initReactI18next).init({
    lng: "en",
    fallbackLng: "en",
    defaultNS: "common",
    nsSeparator: ":",
    keySeparator: ".",
    interpolation: { escapeValue: false },
    react: { useSuspense: false },
    resources: {
      en: {
        common: enCommon,
        tasks: enTasks,
        settings: enSettings,
        errors: enErrors,
        notifications: enNotifications,
        goals: enGoals,
      },
    },
  });
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

// jsdom lacks Pointer Capture APIs; Sonner's toast attaches onPointerDown
// handlers that call setPointerCapture during tests that click toasts.
if (!Element.prototype.setPointerCapture) {
  Element.prototype.setPointerCapture = function setPointerCapture() {};
}
if (!Element.prototype.releasePointerCapture) {
  Element.prototype.releasePointerCapture = function releasePointerCapture() {};
}
if (!Element.prototype.hasPointerCapture) {
  Element.prototype.hasPointerCapture = function hasPointerCapture() {
    return false;
  };
}

// jsdom lacks Element.scrollIntoView; Radix Select scrolls items on open.
if (!Element.prototype.scrollIntoView) {
  Element.prototype.scrollIntoView = function scrollIntoView() {};
}

// jsdom lacks window.matchMedia; components use it for theme detection.
Object.defineProperty(window, "matchMedia", {
  writable: true,
  value: (query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: vi.fn(),
    removeListener: vi.fn(),
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    dispatchEvent: vi.fn(),
  }),
});
