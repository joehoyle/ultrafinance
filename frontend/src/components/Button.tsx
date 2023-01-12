import { ReactNode, useState } from "react";

const styles = {
	primary: "bg-purple-100 text-purple-900 dark:bg-purple-300",
	danger: "bg-red-100 text-red-900",
	secondary: "bg-gray-100 text-gray-900",
}
interface Props {
	children: ReactNode,
	varient?: keyof typeof styles,
	isLoading?: boolean,
}

export default function Button( { isLoading, children, className, varient = "primary", ...props } : Props & React.HTMLProps<HTMLButtonElement> ) {
	const [ confirmed, setConfirmed ] = useState(false);

	function onClick(e: React.MouseEvent<HTMLButtonElement, MouseEvent> ) {
		if ( varient === 'danger' && confirmed === false ) {
			e.preventDefault();
			return setConfirmed( true );
		}
		if ( props.onClick ) {
			props.onClick(e);
		}
	}

	return <button className={`text-xs px-3 py-2 ${ styles[ varient ] } font-medium rounded flex items-center space-x-1 active:translate-y-0.5 ${ className } disabled:opacity-50`} { ...props } onClick={ onClick }>
		{ isLoading && <svg aria-hidden="true" className="w-3 h-3 mr-2 animate-spin fill-blue-600" viewBox="0 0 100 101" fill="none" xmlns="http://www.w3.org/2000/svg">
						<path d="M93.9676 39.0409C96.393 38.4038 97.8624 35.9116 97.0079 33.5539C95.2932 28.8227 92.871 24.3692 89.8167 20.348C85.8452 15.1192 80.8826 10.7238 75.2124 7.41289C69.5422 4.10194 63.2754 1.94025 56.7698 1.05124C51.7666 0.367541 46.6976 0.446843 41.7345 1.27873C39.2613 1.69328 37.813 4.19778 38.4501 6.62326C39.0873 9.04874 41.5694 10.4717 44.0505 10.1071C47.8511 9.54855 51.7191 9.52689 55.5402 10.0491C60.8642 10.7766 65.9928 12.5457 70.6331 15.2552C75.2735 17.9648 79.3347 21.5619 82.5849 25.841C84.9175 28.9121 86.7997 32.2913 88.1811 35.8758C89.083 38.2158 91.5421 39.6781 93.9676 39.0409Z" fill="currentColor" />
					</svg>
					}
			{ varient === 'danger' && confirmed ?
				'Are you sure?' :
			children }
		</button>
}
