import { useMemo } from "react";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

/**
 * IANA timezone picker backed by the browser's own registry
 * (`Intl.supportedValuesOf`). Value `"none"` represents "unset" and maps to
 * null at the API boundary.
 */
export const TIMEZONE_NONE = "none";

export function TimezoneSelect({
  id,
  value,
  onChange,
  ariaInvalid,
}: {
  id: string;
  value: string;
  onChange: (value: string) => void;
  ariaInvalid?: boolean;
}) {
  const zones = useMemo(() => {
    try {
      const supported = (
        Intl as unknown as { supportedValuesOf?: (key: "timeZone") => string[] }
      ).supportedValuesOf;
      return supported ? supported("timeZone") : [];
    } catch {
      return [];
    }
  }, []);

  return (
    <Select value={value || TIMEZONE_NONE} onValueChange={onChange}>
      <SelectTrigger id={id} aria-invalid={ariaInvalid}>
        <SelectValue placeholder="Choose a timezone" />
      </SelectTrigger>
      <SelectContent className="max-h-64">
        <SelectItem value={TIMEZONE_NONE}>
          None — absolute timestamps use the server zone
        </SelectItem>
        {zones.map((zone) => (
          <SelectItem key={zone} value={zone}>
            {zone}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

/** Select value → API value ("none" → null). */
export function timezoneValue(selectValue: string): string | null {
  return selectValue === TIMEZONE_NONE ? null : selectValue;
}
