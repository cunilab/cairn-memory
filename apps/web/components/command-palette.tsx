"use client";

import { useEffect, useRef, useState } from "react";
import { useRouter } from "next/navigation";
import {
  FolderIcon,
  KeyRoundIcon,
  LayoutDashboardIcon,
  LogOutIcon,
  MonitorIcon,
  MoonIcon,
  SunIcon,
  UsersIcon,
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
import { cairn } from "@/lib/cairn";
import { THEMES } from "@/lib/theme";
import type { ListedProject } from "@/lib/types";

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
  const [projects, setProjects] = useState<ListedProject[]>([]);
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
    cairn<ListedProject[]>("projects")
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
          <CommandItem value="overview dashboard" onSelect={() => go("/dashboard")}>
            <LayoutDashboardIcon />
            Overview
          </CommandItem>
          <CommandItem value="projects" onSelect={() => go("/projects")}>
            <FolderIcon />
            Projects
          </CommandItem>
          <CommandItem value="devices tokens" onSelect={() => go("/devices")}>
            <KeyRoundIcon />
            Devices
          </CommandItem>
          {account?.is_admin ? (
            <CommandItem value="accounts users" onSelect={() => go("/accounts")}>
              <UsersIcon />
              Accounts
            </CommandItem>
          ) : null}
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
