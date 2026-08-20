/*
 * Copyright © 2025-2026 rustmailer.com
 * Licensed under RustMailer License Agreement v1.0
 * Unauthorized use or distribution is prohibited.
 */

import { ColumnDef } from '@tanstack/react-table'
import { cn } from '@/lib/utils'
import { Checkbox } from '@/components/ui/checkbox'
import LongText from '@/components/long-text'
import { OAuth2Entity } from '../data/schema'
import { DataTableColumnHeader } from './data-table-column-header'
import { DataTableRowActions } from './data-table-row-actions'
import { format } from 'date-fns'
import { EnableAction } from './enable-action'

export const columns: ColumnDef<OAuth2Entity>[] = [
  {
    id: 'select',
    header: ({ table }) => (
      <Checkbox
        checked={
          table.getIsAllPageRowsSelected() ||
          (table.getIsSomePageRowsSelected() && 'indeterminate')
        }
        onCheckedChange={(value) => table.toggleAllPageRowsSelected(!!value)}
        aria-label='Select all'
        className='translate-y-[2px]'
      />
    ),
    meta: {
      className: cn(
        'sticky md:table-cell left-0 z-10 rounded-tl',
        'bg-background transition-colors duration-200 group-hover/row:bg-muted group-data-[state=selected]/row:bg-muted'
      ),
    },
    cell: ({ row }) => (
      <Checkbox
        checked={row.getIsSelected()}
        onCheckedChange={(value) => row.toggleSelected(!!value)}
        aria-label='Select row'
        className='translate-y-[2px]'
      />
    ),
    enableSorting: false,
    enableHiding: false,
  },
  {
    accessorKey: 'id',
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title='Id' />
    ),
    cell: ({ row }) => {
      return <LongText className='max-w-[200px]'>{row.original.id}</LongText>
    },
    meta: { className: 'max-w-[200px]' },
    enableHiding: false,
    enableSorting: false,
  },
  {
    accessorKey: 'enabled',
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title='Enabled' className='ml-4' />
    ),
    cell: EnableAction,
    meta: { className: 'w-8 text-center' },
  },
  {
    accessorKey: 'grant_type',
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title='Grant Type' />
    ),
    cell: ({ row }) => {
      const grantType = row.original.grant_type;
      const label = grantType === 'ClientCredentials' ? 'Client Credentials' : 'Auth Code';
      return (
        <span className={cn(
          'inline-flex items-center rounded-full px-2.5 py-0.5 text-xs font-semibold',
          grantType === 'ClientCredentials'
            ? 'bg-purple-100 text-purple-700 dark:bg-purple-900/30 dark:text-purple-400'
            : 'bg-blue-100 text-blue-700 dark:bg-blue-900/30 dark:text-blue-400'
        )}>
          {label}
        </span>
      );
    },
    meta: { className: 'w-32' },
    enableHiding: true,
    enableSorting: false,
  },
  {
    accessorKey: 'use_proxy',
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title='Use Proxy' className='ml-4' />
    ),
    cell: ({ row }) => {
      const enabled = row.original.use_proxy;
      if (enabled) {
        return <Checkbox className='max-w-8' checked disabled />
      } else {
        return <Checkbox className='max-w-8' disabled />
      }
    },
    meta: { className: 'w-8 text-center' },
  },
  {
    accessorKey: 'auth_url',
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title='Auth Url' />
    ),
    cell: ({ row }) => {
      return <LongText className='max-w-[200px]'>{row.original.auth_url}</LongText>
    },
    meta: { className: 'max-w-[200px]' },
    enableHiding: false,
    enableSorting: false,
  },
  {
    accessorKey: 'token_url',
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title='Token Url' />
    ),
    cell: ({ row }) => {
      return <LongText className='max-w-[200px]'>{row.original.token_url}</LongText>
    },
    meta: { className: 'max-w-[200px]' },
    enableHiding: false,
    enableSorting: false,
  },
  {
    accessorKey: 'redirect_uri',
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title='Redirect Url' />
    ),
    cell: ({ row }) => {
      return <LongText className='max-w-[200px]'>{row.original.redirect_uri}</LongText>
    },
    meta: { className: 'max-w-[200px]' },
    enableHiding: false,
    enableSorting: false,
  },
  {
    accessorKey: 'description',
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title='Description' />
    ),
    cell: ({ row }) => (
      <LongText className='max-w-[200px]'>{row.original.description}</LongText>
    ),
    meta: { className: 'max-w-[200px]' },
    enableHiding: true,
    enableSorting: false
  },
  {
    accessorKey: 'created_at',
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title='Created At' />
    ),
    cell: ({ row }) => {
      const created_at = row.original.created_at;
      const date = format(new Date(created_at), 'yyyy-MM-dd HH:mm:ss');
      return <LongText className='max-w-36'>{date}</LongText>;
    },
    meta: { className: 'w-36' },
    enableHiding: false,
  },
  {
    accessorKey: 'updated_at',
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title='Updated At' />
    ),
    cell: ({ row }) => {
      const updated_at = row.original.updated_at;
      const date = format(new Date(updated_at), 'yyyy-MM-dd HH:mm:ss');
      return <LongText className='max-w-36'>{date}</LongText>;
    },
    meta: { className: 'w-36' },
    enableHiding: false,
  },
  {
    id: 'actions',
    cell: DataTableRowActions,
  },
]
