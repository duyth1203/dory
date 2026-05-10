import { Menu, BookOpen, Settings, HelpCircle, Info } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
  DropdownMenuLabel,
} from "@/components/ui/dropdown-menu";

interface Props {
  onOpenIntro: () => void;
}

export function AppMenu({ onOpenIntro }: Props) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon" className="h-8 w-8" title="Menu">
          <Menu className="h-4 w-4" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-44">
        <DropdownMenuLabel className="text-xs text-muted-foreground font-normal">
          Meeting Note Taker
        </DropdownMenuLabel>
        <DropdownMenuSeparator />
        <DropdownMenuItem onClick={onOpenIntro} className="gap-2 cursor-pointer">
          <BookOpen className="h-4 w-4 text-muted-foreground" />
          Introduction
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem disabled className="gap-2">
          <Settings className="h-4 w-4 text-muted-foreground" />
          Settings
        </DropdownMenuItem>
        <DropdownMenuItem disabled className="gap-2">
          <HelpCircle className="h-4 w-4 text-muted-foreground" />
          Help
        </DropdownMenuItem>
        <DropdownMenuItem disabled className="gap-2">
          <Info className="h-4 w-4 text-muted-foreground" />
          About
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
