import { useEffect, useState } from "react";

export type Theme = "dark" | "light";
export type ThemeChoice = Theme | "system";

const KEY = "cyberbrain.theme";

const systemTheme = (): Theme => (window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark");

function readChoice(): ThemeChoice {
  try {
    const v = localStorage.getItem(KEY);
    return v === "dark" || v === "light" ? v : "system";
  } catch {
    return "system";
  }
}

function apply(choice: ThemeChoice) {
  document.documentElement.dataset.theme = choice === "system" ? systemTheme() : choice;
}

export function useTheme(): { choice: ThemeChoice; effective: Theme; setChoice: (c: ThemeChoice) => void; cycle: () => void } {
  const [choice, setChoiceState] = useState<ThemeChoice>(readChoice);
  const [effective, setEffective] = useState<Theme>(() => (choice === "system" ? systemTheme() : choice));

  useEffect(() => {
    apply(choice);
    setEffective(choice === "system" ? systemTheme() : choice);
    const mq = window.matchMedia("(prefers-color-scheme: light)");
    const on = () => {
      if (choice === "system") {
        apply("system");
        setEffective(systemTheme());
      }
    };
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, [choice]);

  const setChoice = (c: ThemeChoice) => {
    try {
      if (c === "system") localStorage.removeItem(KEY);
      else localStorage.setItem(KEY, c);
    } catch {
      /* storage may be unavailable; the choice still applies for this page */
    }
    setChoiceState(c);
  };
  const cycle = () => setChoice(choice === "system" ? (systemTheme() === "dark" ? "light" : "dark") : choice === "dark" ? "light" : "system");
  return { choice, effective, setChoice, cycle };
}
