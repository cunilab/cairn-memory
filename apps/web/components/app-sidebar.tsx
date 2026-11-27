"use client";

import Link from "next/link";
import { useEffect, useState } from "react";
import { usePathname } from "next/navigation";
import {
  ChevronsUpDownIcon,
  ActivityIcon,
  BrainIcon,
  BookOpenIcon,
  FolderIcon,
  LayoutDashboardIcon,
  LockKeyholeIcon,
  LogOutIcon,
  MonitorIcon,
  MoonIcon,
  SettingsIcon,
  SunIcon,
  WaypointsIcon,
  WrenchIcon,
} from "lucide-react";
import { CairnMark } from "@/components/cairn-logo";
import { Avatar, AvatarFallback } from "@/components/ui/avatar";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuBadge,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarRail,
  useSidebar,
} from "@/components/ui/sidebar";
import { Skeleton } from "@/components/ui/skeleton";
import { useAccount } from "@/hooks/use-account";
import { useAttention } from "@/hooks/use-attention";
import { useLogout } from "@/hooks/use-logout";
import { useTheme } from "@/hooks/use-theme";
import { THEMES, type Theme } from "@/lib/theme";
import { cairn } from "@/lib/cairn";

const THEME_ICONS = { light: SunIcon, dark: MoonIcon, system: MonitorIcon };

const WORKSPACE = [
  { href: "/dashboard", label: "Overview", icon: LayoutDashboardIcon },
  { href: "/memories", label: "Memories", icon: BrainIcon },
  { href: "/projects", label: "Projects", icon: FolderIcon },
  { href: "/skills", label: "Skills", icon: BookOpenIcon },
  { href: "/activity", label: "Activity", icon: ActivityIcon },
];

function NavItem({
  href,
  label,
  icon: Icon,
  pathname,
  attention,
  count,
}: {
  href: string;
  label: string;
  icon: typeof FolderIcon;
  pathname: string;
  attention?: string;
  count?: number;
}) {
  const { setOpenMobile } = useSidebar();
  return (
    <SidebarMenuItem>
      <SidebarMenuButton
        isActive={pathname === href || pathname.startsWith(`${href}/`)}
        tooltip={label}
        render={<Link href={href} onClick={() => setOpenMobile(false)} />}
      >
        <Icon />
        <span>{label}</span>
      </SidebarMenuButton>
      {count !== undefined ? <SidebarMenuBadge aria-label={`${count} pending fixes`}>{count}</SidebarMenuBadge> : attention ? <SidebarMenuBadge aria-label={attention}>•</SidebarMenuBadge> : null}
    </SidebarMenuItem>
  );
}

function AccountMenu() {
  const account = useAccount();
  const logout = useLogout();
  const [theme, setTheme] = useTheme();
  if (account === undefined) {
    return <Skeleton className="h-10 w-full" />;
  }
  const email = account?.email ?? "Account";
  const ThemeIcon = THEME_ICONS[theme];
  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger
          render={
            <SidebarMenuButton
              size="lg"
              tooltip={email}
              className="data-popup-open:bg-sidebar-accent"
            />
          }
        >
          <Avatar className="size-7 rounded-md">
            <AvatarFallback className="rounded-md bg-primary/20 text-xs font-medium text-brand">
              {email.slice(0, 1).toUpperCase()}
            </AvatarFallback>
          </Avatar>
          <span className="grid min-w-0 flex-1 text-left leading-tight">
            <span className="truncate text-sm font-medium">{email}</span>
            <span className="truncate text-xs text-muted-foreground">
              {account?.is_admin ? "Admin" : "Account"}
            </span>
          </span>
          <ChevronsUpDownIcon className="ml-auto size-4 text-muted-foreground" />
        </DropdownMenuTrigger>
        <DropdownMenuContent side="top" align="start" className="w-60">
          <DropdownMenuGroup>
            <DropdownMenuLabel className="truncate">{email}</DropdownMenuLabel>
          </DropdownMenuGroup>
          <DropdownMenuSeparator />
          <DropdownMenuItem render={<Link href="/settings" />}>
            <SettingsIcon />
            Settings
          </DropdownMenuItem>
          <DropdownMenuItem render={<Link href="/settings?tab=account" />}>
            <LockKeyholeIcon />
            Change password
          </DropdownMenuItem>
          <DropdownMenuSub>
            <DropdownMenuSubTrigger>
              <ThemeIcon />
              Theme
            </DropdownMenuSubTrigger>
            <DropdownMenuSubContent>
              <DropdownMenuRadioGroup
                value={theme}
                onValueChange={(value) => setTheme(value as Theme)}
              >
                {THEMES.map((item) => (
                  <DropdownMenuRadioItem key={item.value} value={item.value}>
                    {item.label}
                  </DropdownMenuRadioItem>
                ))}
              </DropdownMenuRadioGroup>
            </DropdownMenuSubContent>
          </DropdownMenuSub>
          <DropdownMenuSeparator />
          <DropdownMenuItem onClick={() => void logout()}>
            <LogOutIcon />
            Log out
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </>
  );
}

export function AppSidebar({ ...props }: React.ComponentProps<typeof Sidebar>) {
  const pathname = usePathname();
  const attention = useAttention();
  const account = useAccount();
  const [fixCount, setFixCount] = useState(0);
  useEffect(() => {
    if (!account?.is_admin) return;
    let cancelled = false;
    const refresh = () => void cairn<{ count: number }>("admin/fixes/count").then((result) => { if (!cancelled) setFixCount(result.count); }).catch(() => {});
    refresh();
    const timer = setInterval(() => { if (document.visibilityState === "visible") refresh(); }, 60_000);
    window.addEventListener("cairn-fixes-changed", refresh);
    return () => { cancelled = true; clearInterval(timer); window.removeEventListener("cairn-fixes-changed", refresh); };
  }, [account]);
  return (
    <Sidebar collapsible="icon" {...props}>
      <SidebarHeader>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton size="lg" tooltip="Cairn" render={<Link href="/dashboard" />}>
              <CairnMark className="size-7!" />
              <span className="text-[1.0625rem] font-semibold tracking-[-0.02em]">cairn</span>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarHeader>
      <SidebarContent>
        <SidebarGroup>
          <SidebarGroupContent>
            <SidebarMenu>
              {WORKSPACE.map((item) => (
                <NavItem key={item.href} {...item} pathname={pathname} attention={attention[item.href]} />
              ))}
            </SidebarMenu>
          </SidebarGroupContent>
        </SidebarGroup>
        <SidebarGroup>
          <SidebarGroupContent>
            <SidebarMenu>
              <NavItem href="/graph" label="Relationships" icon={WaypointsIcon} pathname={pathname} />
              {account?.is_admin ? <NavItem href="/fixes" label="Fixes" icon={WrenchIcon} pathname={pathname} count={fixCount} /> : null}
            </SidebarMenu>
          </SidebarGroupContent>
        </SidebarGroup>
      </SidebarContent>
      <SidebarFooter>
        <SidebarMenu>
          <SidebarMenuItem>
            <AccountMenu />
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarFooter>
      <SidebarRail />
    </Sidebar>
  );
}
