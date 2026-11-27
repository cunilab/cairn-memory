"use client";

import { useEffect, useRef, useState } from "react";
import { useRouter } from "next/navigation";
import {
  ActivityIcon,
  BookOpenIcon,
  BrainIcon,
  DownloadIcon,
  FolderIcon,
  KeyRoundIcon,
  LayoutDashboardIcon,
  LockKeyholeIcon,
  LogOutIcon,
  MonitorIcon,
  MoonIcon,
  PinIcon,
  SettingsIcon,
  SunIcon,
  WaypointsIcon,
  WrenchIcon,
} from "lucide-react";
import {
  Command,
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandSeparator,
} from "@/components/ui/command";
import { useAccount } from "@/hooks/use-account";
import { useLogout } from "@/hooks/use-logout";
import { useTheme } from "@/hooks/use-theme";
import { toast } from "sonner";
import { cairn } from "@/lib/cairn";
import { downloadExport } from "@/lib/export";
import { THEMES } from "@/lib/theme";
import type { ProjectView } from "@/lib/generated/ProjectView";

const PAGES = [
  { href: "/dashboard", label: "Overview", keywords: "dashboard home", icon: LayoutDashboardIcon },
  { href: "/memories", label: "Memories", keywords: "search list", icon: BrainIcon },
  { href: "/memories?view=pinned", label: "Pinned memories", keywords: "pins", icon: PinIcon },
  { href: "/projects", label: "Projects", keywords: "repositories branches", icon: FolderIcon },
  { href: "/skills", label: "Skills", keywords: "library procedures", icon: BookOpenIcon },
  { href: "/activity", label: "Activity", keywords: "timeline audit jobs events", icon: ActivityIcon },
  { href: "/graph", label: "Relationships", keywords: "graph links", icon: WaypointsIcon },
  { href: "/settings", label: "Settings", keywords: "devices tokens accounts password", icon: SettingsIcon },
];

const THEME_ICONS = { light: SunIcon, dark: MoonIcon, system: MonitorIcon };

/** Jump to a page or project with Cmd+K (Ctrl+K elsewhere). Projects load on first open. */
export function CommandPalette({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const router = useRouter();
  const account = useAccount();
  const logout = useLogout();
  const [, setTheme] = useTheme();
  const [projects, setProjects] = useState<ProjectView[]>([]);
  const loaded = useRef(false);

  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      if (event.key.toLowerCase() === "k" && (event.metaKey || event.ctrlKey)) {
        event.preventDefault();
        onOpenChange(!open);
      }
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open, onOpenChange]);

  useEffect(() => {
    if (!open || loaded.current) {
      return;
    }
    loaded.current = true;
    cairn<ProjectView[]>("projects")
      .then(setProjects)
      .catch(() => {
        loaded.current = false;
      });
  }, [open]);

  function go(href: string) {
    onOpenChange(false);
    router.push(href);
  }

  return (
    <CommandDialog
      open={open}
      onOpenChange={onOpenChange}
      title="Jump to"
      description="Search pages and projects."
    >
      <Command>
      <CommandInput placeholder="Jump to a page or project…" />
      <CommandList>
        <CommandEmpty>Nothing matches.</CommandEmpty>
        <CommandGroup heading="Pages">
          {PAGES.map(({ href, label, keywords, icon: Icon }) => (
            <CommandItem key={href} value={`${label} ${keywords}`} onSelect={() => go(href)}>
              <Icon />
              {label}
            </CommandItem>
          ))}
          {account?.is_admin ? (
            <CommandItem value="fixes admin queue" onSelect={() => go("/fixes")}>
              <WrenchIcon />
              Fixes
            </CommandItem>
          ) : null}
        </CommandGroup>
        <CommandGroup heading="Actions">
          <CommandItem
            value="export memories download"
            onSelect={() => {
              onOpenChange(false);
              downloadExport().then(
                () => toast.success("Export ready"),
                (cause: unknown) => toast.error(cause instanceof Error ? cause.message : "Could not export."),
              );
            }}
          >
            <DownloadIcon />
            Export memories
          </CommandItem>
          <CommandItem value="create device token setup agent" onSelect={() => go("/settings?tab=devices&create=1")}>
            <KeyRoundIcon />
            Create device token
          </CommandItem>
          <CommandItem value="change password" onSelect={() => go("/settings?tab=account")}>
            <LockKeyholeIcon />
            Change password
          </CommandItem>
        </CommandGroup>
        {projects.length > 0 ? (
          <CommandGroup heading="Projects">
            {projects.map((project) => (
              <CommandItem
                key={project.project_id}
                value={`${project.name} ${project.repository} ${project.project_id}`}
                onSelect={() => go(`/projects/${project.project_id}`)}
              >
                <FolderIcon />
                <span className="truncate">{project.name}</span>
                <span className="ml-auto truncate text-xs text-muted-foreground">
                  {project.repository}
                </span>
              </CommandItem>
            ))}
          </CommandGroup>
        ) : null}
        <CommandSeparator />
        <CommandGroup heading="Theme">
          {THEMES.map((item) => {
            const Icon = THEME_ICONS[item.value];
            return (
              <CommandItem
                key={item.value}
                value={`theme ${item.label}`}
                onSelect={() => {
                  setTheme(item.value);
                  onOpenChange(false);
                }}
              >
                <Icon />
                {item.label}
              </CommandItem>
            );
          })}
        </CommandGroup>
        <CommandSeparator />
        <CommandGroup heading="Account">
          <CommandItem
            value="log out sign out"
            onSelect={() => {
              onOpenChange(false);
              void logout();
            }}
          >
            <LogOutIcon />
            Log out
          </CommandItem>
        </CommandGroup>
      </CommandList>
      </Command>
    </CommandDialog>
  );
}
