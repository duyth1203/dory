import { Check, Monitor, Moon, Menu, BookOpen, Settings, HelpCircle, Info, Sun } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
  DropdownMenuLabel,
} from "@/components/ui/dropdown-menu";
import { useTheme, type ThemePreference } from "@/contexts/theme";

interface Props {
  readonly onOpenIntro: () => void;
}

function AppearanceItem({
  value,
  current,
  onSelect,
  icon: Icon,
  label,
}: Readonly<{
  value: ThemePreference;
  current: ThemePreference;
  onSelect: (v: ThemePreference) => void;
  icon: typeof Sun;
  label: string;
}>) {
  return (
    <DropdownMenuItem className="gap-2 justify-between cursor-pointer" onClick={() => onSelect(value)}>
      <span className="flex items-center gap-2">
        <Icon className="h-4 w-4 text-muted-foreground" aria-hidden />
        {label}
      </span>
      {current === value ? <Check className="h-4 w-4 shrink-0 text-primary" aria-hidden /> : null}
    </DropdownMenuItem>
  );
}

export function AppMenu({ onOpenIntro }: Props) {
  const { preference, setPreference } = useTheme();

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon" className="h-8 w-8" title="Menu">
          <Menu className="h-4 w-4" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-52">
        <DropdownMenuLabel className="text-xs text-muted-foreground font-normal">
          Meeting Note Taker
        </DropdownMenuLabel>
        <DropdownMenuSeparator />
        <DropdownMenuItem onClick={onOpenIntro} className="gap-2 cursor-pointer">
          <BookOpen className="h-4 w-4 text-muted-foreground" />
          Introduction
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuLabel className="text-xs text-muted-foreground font-normal">Appearance</DropdownMenuLabel>
        <AppearanceItem
          value="light"
          current={preference}
          onSelect={setPreference}
          icon={Sun}
          label="Light"
        />
        <AppearanceItem
          value="dark"
          current={preference}
          onSelect={setPreference}
          icon={Moon}
          label="Dark"
        />
        <AppearanceItem
          value="system"
          current={preference}
          onSelect={setPreference}
          icon={Monitor}
          label="System"
        />
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
