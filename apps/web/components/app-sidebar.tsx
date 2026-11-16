"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { useState } from "react";
import {
  ChevronsUpDownIcon,
  FolderIcon,
  KeyRoundIcon,
  LayoutDashboardIcon,
  LockKeyholeIcon,
  LogOutIcon,
  MonitorIcon,
  MoonIcon,
  SunIcon,
  UsersIcon,
} from "lucide-react";
import { CairnMark } from "@/components/cairn-logo";
import { ChangePasswordDialog } from "@/components/change-password-dialog";
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
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarRail,
  useSidebar,
} from "@/components/ui/sidebar";
import { Skeleton } from "@/components/ui/skeleton";
import { useAccount } from "@/hooks/use-account";
import { useLogout } from "@/hooks/use-logout";
import { useTheme } from "@/hooks/use-theme";
import { THEMES, type Theme } from "@/lib/theme";

const THEME_ICONS = { light: SunIcon, dark: MoonIcon, system: MonitorIcon };

const WORKSPACE = [
  { href: "/dashboard", label: "Overview", icon: LayoutDashboardIcon },
  { href: "/projects", label: "Projects", icon: FolderIcon },
  { href: "/devices", label: "Devices", icon: KeyRoundIcon },
];

function NavItem({
  href,
  label,
  icon: Icon,
  pathname,
}: {
  href: string;
  label: string;
  icon: typeof FolderIcon;
  pathname: string;
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
    </SidebarMenuItem>
  );
}

function AccountMenu() {
  const account = useAccount();
  const logout = useLogout();
  const [theme, setTheme] = useTheme();
  const [changingPassword, setChangingPassword] = useState(false);
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
          <DropdownMenuItem onClick={() => setChangingPassword(true)}>
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
      <ChangePasswordDialog open={changingPassword} onOpenChange={setChangingPassword} />
    </>
  );
}

export function AppSidebar({ ...props }: React.ComponentProps<typeof Sidebar>) {
  const account = useAccount();
  const pathname = usePathname();
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
          <SidebarGroupLabel>Workspace</SidebarGroupLabel>
          <SidebarGroupContent>
            <SidebarMenu>
              {WORKSPACE.map((item) => (
                <NavItem key={item.href} {...item} pathname={pathname} />
              ))}
            </SidebarMenu>
          </SidebarGroupContent>
        </SidebarGroup>
        {account?.is_admin ? (
          <SidebarGroup>
            <SidebarGroupLabel>Admin</SidebarGroupLabel>
            <SidebarGroupContent>
              <SidebarMenu>
                <NavItem href="/accounts" label="Accounts" icon={UsersIcon} pathname={pathname} />
              </SidebarMenu>
            </SidebarGroupContent>
          </SidebarGroup>
        ) : null}
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
