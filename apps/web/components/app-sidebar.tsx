"use client";

import Link from "next/link";
import { FolderIcon, KeyRoundIcon, LayoutDashboardIcon, UsersIcon } from "lucide-react";
import {
  Sidebar,
  SidebarContent,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
} from "@/components/ui/sidebar";
import { useAccount } from "@/hooks/use-account";

export function AppSidebar({ ...props }: React.ComponentProps<typeof Sidebar>) {
  const account = useAccount();
  return (
    <Sidebar collapsible="offcanvas" {...props}>
      <SidebarHeader>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton
              className="data-[slot=sidebar-menu-button]:p-1.5!"
              render={<Link href="/projects" />}
            >
              <FolderIcon className="size-5!" />
              <span className="text-base font-semibold">Cairn</span>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarHeader>
      <SidebarContent>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton render={<Link href="/dashboard" />}>
              <LayoutDashboardIcon />
              <span>Overview</span>
            </SidebarMenuButton>
          </SidebarMenuItem>
          <SidebarMenuItem>
            <SidebarMenuButton render={<Link href="/projects" />}>
              <FolderIcon />
              <span>Projects</span>
            </SidebarMenuButton>
          </SidebarMenuItem>
          <SidebarMenuItem>
            <SidebarMenuButton render={<Link href="/devices" />}>
              <KeyRoundIcon />
              <span>Devices</span>
            </SidebarMenuButton>
          </SidebarMenuItem>
          {account?.is_admin ? (
            <SidebarMenuItem>
              <SidebarMenuButton render={<Link href="/accounts" />}>
                <UsersIcon />
                <span>Accounts</span>
              </SidebarMenuButton>
            </SidebarMenuItem>
          ) : null}
        </SidebarMenu>
      </SidebarContent>
    </Sidebar>
  );
}
