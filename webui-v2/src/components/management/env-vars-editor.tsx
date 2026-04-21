import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

export interface EnvVar {
  key: string;
  value: string;
}

interface EnvVarsEditorProps {
  value: EnvVar[];
  onChange: (vars: EnvVar[]) => void;
}

export function EnvVarsEditor({ value, onChange }: EnvVarsEditorProps) {
  return (
    <div className="space-y-2">
      {value.map((pair, i) => (
        <div key={i} className="flex gap-2">
          <Input
            placeholder="KEY"
            value={pair.key}
            onChange={(e) => {
              const updated = [...value];
              updated[i] = { ...updated[i], key: e.target.value };
              onChange(updated);
            }}
            className="flex-1 font-mono text-xs"
          />
          <Input
            placeholder="value"
            value={pair.value}
            onChange={(e) => {
              const updated = [...value];
              updated[i] = { ...updated[i], value: e.target.value };
              onChange(updated);
            }}
            className="flex-1 font-mono text-xs"
          />
          <Button
            type="button"
            variant="ghost"
            size="sm"
            onClick={() => onChange(value.filter((_, j) => j !== i))}
            className="px-2 text-muted-foreground hover:text-destructive"
          >
            ×
          </Button>
        </div>
      ))}
      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={() => onChange([...value, { key: "", value: "" }])}
      >
        + Add Variable
      </Button>
    </div>
  );
}
