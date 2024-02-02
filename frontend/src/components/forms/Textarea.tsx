
interface Props {

}

export default function Textarea({ ...props }: Props & React.HTMLProps<HTMLTextAreaElement>) {
	const className = `text-xs text-gray-700 border rounded-sm min-w-[250px] border-gray-200 dark:border-transparent dark:rounded dark:bg-white/10 dark:text-gray-300 px-2 py-2 outline-none focus:border-purple-400 focus:dark:border-purple-400 ${props.className}`;
	props = {
		...props,
		className,
	}
	return <textarea
		{...props}
	></textarea>
}
