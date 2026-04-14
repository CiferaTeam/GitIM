import { cn } from "@/lib/utils"

function Input({ className, ...props }: React.ComponentProps<"input">) {
  return (
    <input
      className={cn(
        "w-full rounded-lg border border-input bg-surface px-4 py-2.5 text-sm text-foreground placeholder:text-text-faint outline-none transition-colors focus:border-ring",
        className
      )}
      {...props}
    />
  )
}

export { Input }
