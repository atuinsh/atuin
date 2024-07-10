import type { ClassValue } from "clsx";

import clsx from "clsx";
import { extendTailwindMerge } from "tailwind-merge";

const COMMON_UNITS = ["small", "medium", "large"];

/**
 * We need to extend the tailwind merge to include NextUI's custom classes.
 *
 * So we can use classes like `text-small` or `text-default-500` and override them.
 */
const twMerge = extendTailwindMerge({
  extend: {
    theme: {
      opacity: ["disabled"],
      spacing: ["divider"],
      borderWidth: COMMON_UNITS,
      borderRadius: COMMON_UNITS,
    },
    classGroups: {
      shadow: [{ shadow: COMMON_UNITS }],
      "font-size": [{ text: ["tiny", ...COMMON_UNITS] }],
      "bg-image": ["bg-stripe-gradient"],
    },
  },
});

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

// edge still uses the old one
export function getWeekInfo() {
  let locale = new Intl.Locale(navigator.language);

  // @ts-ignore
  if (locale.getWeekInfo) {
    // @ts-ignore
    return locale.getWeekInfo();
    // @ts-ignore
  } else if (locale.weekInfo) {
    // @ts-ignore
    return locale.weekInfo;
  }

  throw new Error("Could not fetch week info via new or old api");
}
