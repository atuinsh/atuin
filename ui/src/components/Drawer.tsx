import { Drawer as VDrawer } from "vaul";

export default function Drawer({
  trigger,
  children,
  width,
  open,
  onOpenChange,
}: any) {
  return (
    <VDrawer.Root direction="right" open={open} onOpenChange={onOpenChange}>
      <VDrawer.Trigger asChild>{trigger}</VDrawer.Trigger>
      <VDrawer.Portal>
        <VDrawer.Overlay className="fixed inset-0 bg-black/40 z-50" />
        <VDrawer.Content
          style={{ width: width || "400px" }}
          className={`bg-white flex flex-col z-50 h-full mt-24 fixed bottom-0 right-0`}
        >
          {children}
        </VDrawer.Content>
      </VDrawer.Portal>
    </VDrawer.Root>
  );
}
