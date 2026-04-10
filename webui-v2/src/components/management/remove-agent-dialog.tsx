import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { useAgentStore } from "@/hooks/use-agent-store";
import * as mockClient from "@/lib/mock/client";

interface RemoveAgentDialogProps {
  agentId: string;
  agentName: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Called after the agent has been removed from the store. */
  onRemoved?: () => void;
}

export function RemoveAgentDialog({
  agentId,
  agentName,
  open,
  onOpenChange,
  onRemoved,
}: RemoveAgentDialogProps) {
  const removeAgent = useAgentStore((s) => s.removeAgent);

  async function handleConfirm() {
    const res = await mockClient.removeAgent(agentId);
    if (res.ok) {
      removeAgent(agentId);
      onOpenChange(false);
      onRemoved?.();
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Remove agent?</DialogTitle>
        </DialogHeader>
        <p className="text-sm text-muted-foreground">
          Remove agent <span className="font-medium text-foreground">{agentName}</span>?
          This will stop the agent and remove its configuration.
        </p>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button variant="destructive" onClick={handleConfirm}>
            Remove
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
